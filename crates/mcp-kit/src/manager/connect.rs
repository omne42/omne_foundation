use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use tokio::process::{Child, Command};

use crate::error::{ErrorKind, tagged_message, wrap_kind};
use crate::protocol::{AUTHORIZATION_HEADER, MCP_PROTOCOL_VERSION_HEADER};
use crate::{ServerConfig, Transport, TrustMode, UntrustedStreamableHttpPolicy};

use super::placeholders::{
    apply_stdio_baseline_env, expand_placeholders_trusted, expand_placeholders_trusted_os,
};
use super::streamable_http_validation::{
    validate_streamable_http_config, validate_streamable_http_url_untrusted_dns,
};

const STREAMABLE_HTTP_SESSION_ID_HEADER: &str = "mcp-session-id";

macro_rules! config_bail {
    ($($arg:tt)*) => {
        return Err(tagged_message(ErrorKind::Config, format!($($arg)*)))
    };
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectContext {
    pub(crate) trust_mode: TrustMode,
    pub(crate) untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy,
    pub(crate) allow_stdout_log_outside_root: bool,
    pub(crate) stdout_log_root: Option<PathBuf>,
    pub(crate) protocol_version: String,
    pub(crate) request_timeout: Duration,
}

#[derive(Debug, Clone)]
struct ResolvedStreamableHttpUrls {
    sse_url: String,
    post_url: String,
    sse_url_field: &'static str,
    post_url_field: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionServerConfigIdentity {
    Stdio {
        argv: Vec<OsString>,
        inherit_env: bool,
        env: BTreeMap<String, OsString>,
        stdout_log: Option<ResolvedStdoutLogConfig>,
    },
    Unix {
        unix_path: PathBuf,
    },
    StreamableHttp {
        sse_url: String,
        post_url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedStdoutLogConfig {
    pub(crate) path: PathBuf,
    pub(crate) max_bytes_per_part: u64,
    pub(crate) max_parts: Option<u32>,
}

pub(crate) fn raw_server_config_identity(
    server_cfg: &ServerConfig,
) -> ConnectionServerConfigIdentity {
    match server_cfg {
        ServerConfig::Stdio(_) => ConnectionServerConfigIdentity::Stdio {
            argv: server_cfg
                .argv()
                .iter()
                .cloned()
                .map(OsString::from)
                .collect(),
            inherit_env: server_cfg.inherit_env(),
            env: server_cfg
                .env()
                .iter()
                .map(|(key, value)| (key.clone(), OsString::from(value)))
                .collect(),
            stdout_log: server_cfg.stdout_log().map(|log| ResolvedStdoutLogConfig {
                path: log.path.clone(),
                max_bytes_per_part: log.max_bytes_per_part,
                max_parts: log.max_parts,
            }),
        },
        ServerConfig::Unix(_) => ConnectionServerConfigIdentity::Unix {
            unix_path: server_cfg.unix_path_required().to_path_buf(),
        },
        ServerConfig::StreamableHttp(_) => {
            let mut headers = BTreeMap::new();
            headers.extend(
                server_cfg
                    .http_headers()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            if let Some(env_var) = server_cfg.bearer_token_env_var() {
                headers.insert(
                    AUTHORIZATION_HEADER.to_string(),
                    format!("$ENVVAR:{env_var}"),
                );
            }
            for (header, env_var) in server_cfg.env_http_headers() {
                headers.insert(header.clone(), format!("$ENVVAR:{env_var}"));
            }

            let (sse_url, post_url) = match (
                server_cfg.url(),
                server_cfg.sse_url(),
                server_cfg.http_url(),
            ) {
                (Some(url), None, None) => (url.to_string(), url.to_string()),
                (None, Some(sse_url), Some(http_url)) => {
                    (sse_url.to_string(), http_url.to_string())
                }
                _ => ("<invalid>".to_string(), "<invalid>".to_string()),
            };
            ConnectionServerConfigIdentity::StreamableHttp {
                sse_url,
                post_url,
                headers,
            }
        }
    }
}

pub(crate) fn effective_server_config_identity(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<ConnectionServerConfigIdentity> {
    match server_cfg.transport() {
        Transport::Stdio => {
            let cwd = super::resolve_connection_cwd(cwd)?;
            let argv = server_cfg
                .argv()
                .iter()
                .enumerate()
                .map(|(idx, arg)| {
                    expand_placeholders_trusted_os(arg, &cwd).with_context(|| {
                        format!(
                            "expand argv placeholder (server={server_name} argv[{idx}] redacted)"
                        )
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(|err| wrap_kind(ErrorKind::Config, err))?;
            let env = server_cfg
                .env()
                .iter()
                .map(|(key, value)| {
                    expand_placeholders_trusted_os(value, &cwd)
                        .with_context(|| format!("expand env placeholder: {key}"))
                        .map(|value| (key.clone(), value))
                        .map_err(|err| wrap_kind(ErrorKind::Config, err))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            let stdout_log = server_cfg.stdout_log().map(|log| ResolvedStdoutLogConfig {
                path: absolutize_with_base(&log.path, &cwd),
                max_bytes_per_part: log.max_bytes_per_part,
                max_parts: log.max_parts,
            });
            Ok(ConnectionServerConfigIdentity::Stdio {
                argv,
                inherit_env: server_cfg.inherit_env(),
                env,
                stdout_log,
            })
        }
        Transport::Unix => Ok(ConnectionServerConfigIdentity::Unix {
            unix_path: resolve_unix_socket_path(server_cfg.unix_path_required(), cwd)?,
        }),
        Transport::StreamableHttp => {
            let resolved_urls = resolve_streamable_http_urls(ctx, server_name, server_cfg, cwd)?;
            let mut headers = build_streamable_http_headers(ctx, server_name, server_cfg, cwd)?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            headers.remove(MCP_PROTOCOL_VERSION_HEADER);
            Ok(ConnectionServerConfigIdentity::StreamableHttp {
                sse_url: resolved_urls.sse_url,
                post_url: resolved_urls.post_url,
                headers,
            })
        }
    }
}

pub(crate) async fn connect_transport(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    match server_cfg.transport() {
        Transport::Stdio => connect_stdio_transport(ctx, server_name, server_cfg, cwd).await,
        Transport::Unix => connect_unix_transport(ctx, server_name, server_cfg, cwd).await,
        Transport::StreamableHttp => {
            connect_streamable_http_transport(ctx, server_name, server_cfg, cwd).await
        }
    }
}

async fn connect_stdio_transport(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    let cwd = super::resolve_connection_cwd(cwd)?;

    if ctx.trust_mode == TrustMode::Untrusted {
        config_bail!(
            "refusing to spawn mcp server in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        );
    }

    let expanded_argv = server_cfg
        .argv()
        .iter()
        .enumerate()
        .map(|(idx, arg)| {
            expand_placeholders_trusted_os(arg, &cwd).with_context(|| {
                format!("expand argv placeholder (server={server_name} argv[{idx}] redacted)")
            })
        })
        .collect::<anyhow::Result<Vec<std::ffi::OsString>>>()
        .map_err(|err| wrap_kind(ErrorKind::Config, err))?;

    let mut cmd = Command::new(&expanded_argv[0]);
    cmd.args(expanded_argv.iter().skip(1));
    cmd.current_dir(&cwd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    // Library callers own stderr routing. Default to /dev/null instead of leaking server logs or
    // secrets into the host process boundary.
    cmd.stderr(Stdio::null());
    if !server_cfg.inherit_env() {
        cmd.env_clear();
        apply_stdio_baseline_env(&mut cmd);
    }
    for (key, value) in server_cfg.env().iter() {
        let value = expand_placeholders_trusted_os(value, &cwd)
            .with_context(|| format!("expand env placeholder: {key}"))
            .map_err(|err| wrap_kind(ErrorKind::Config, err))?;
        cmd.env(key, value);
    }
    cmd.kill_on_drop(true);

    let stdout_log = server_cfg.stdout_log().map(|log| {
        let resolved_log_path = absolutize_with_base(&log.path, &cwd);
        let stdout_log_root = ctx.stdout_log_root.as_deref().unwrap_or(cwd.as_path());
        let within_root = stdout_log_path_within_root(&resolved_log_path, stdout_log_root)
            .map_err(|err| {
                wrap_kind(
                    ErrorKind::Config,
                    anyhow::Error::new(err).context(format!(
                        "resolve stdout_log.path root boundary (server={server_name})"
                    )),
                )
            })?;
        if !ctx.allow_stdout_log_outside_root && !within_root {
            config_bail!(
                "mcp server {server_name}: stdout_log.path must be within root (set Manager::with_allow_stdout_log_outside_root(true) to override): {}",
                log.path.display()
            );
        }
        Ok::<_, anyhow::Error>(mcp_jsonrpc::StdoutLog {
            path: resolved_log_path,
            max_bytes_per_part: log.max_bytes_per_part,
            max_parts: log.max_parts,
        })
    });
    let stdout_log = stdout_log.transpose()?;
    let mut client = mcp_jsonrpc::Client::spawn_command_with_options(
        cmd,
        mcp_jsonrpc::SpawnOptions {
            stdout_log,
            ..Default::default()
        },
    )
    .await
    .with_context(|| {
        format!(
            "spawn mcp server (server={server_name}) argv redacted (argc={})",
            server_cfg.argv().len()
        )
    })?;
    let child = client.take_child();
    Ok((client, child))
}

async fn connect_unix_transport(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    if ctx.trust_mode == TrustMode::Untrusted {
        config_bail!(
            "refusing to connect unix mcp server in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        );
    }
    let unix_path = resolve_unix_socket_path(server_cfg.unix_path_required(), cwd)?;
    let client = mcp_jsonrpc::Client::connect_unix(&unix_path)
        .await
        .with_context(|| format!("connect unix mcp server path={}", unix_path.display()))?;
    Ok((client, None))
}

async fn connect_streamable_http_transport(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    let resolved_urls = resolve_streamable_http_urls(ctx, server_name, server_cfg, cwd)?;

    validate_streamable_http_config(
        ctx.trust_mode,
        &ctx.untrusted_streamable_http_policy,
        server_name,
        resolved_urls.sse_url_field,
        &resolved_urls.sse_url,
        server_cfg,
    )?;
    if resolved_urls.post_url != resolved_urls.sse_url {
        validate_streamable_http_config(
            ctx.trust_mode,
            &ctx.untrusted_streamable_http_policy,
            server_name,
            resolved_urls.post_url_field,
            &resolved_urls.post_url,
            server_cfg,
        )?;
    }

    if ctx.trust_mode != TrustMode::Trusted {
        if server_cfg.bearer_token_env_var().is_some() {
            config_bail!(
                "refusing to read bearer token env var in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }
        if !server_cfg.env_http_headers().is_empty() {
            config_bail!(
                "refusing to read http header env vars in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }

        validate_streamable_http_url_untrusted_dns(
            &ctx.untrusted_streamable_http_policy,
            server_name,
            resolved_urls.sse_url_field,
            &resolved_urls.sse_url,
        )
        .await?;
        if resolved_urls.post_url != resolved_urls.sse_url {
            validate_streamable_http_url_untrusted_dns(
                &ctx.untrusted_streamable_http_policy,
                server_name,
                resolved_urls.post_url_field,
                &resolved_urls.post_url,
            )
            .await?;
        }
    }

    let headers = build_streamable_http_headers(ctx, server_name, server_cfg, cwd)?;
    let client = mcp_jsonrpc::Client::connect_streamable_http_split_with_options(
        &resolved_urls.sse_url,
        &resolved_urls.post_url,
        mcp_jsonrpc::StreamableHttpOptions {
            headers,
            enforce_public_ip: should_enforce_public_ip(ctx),
            request_timeout: Some(ctx.request_timeout),
            ..Default::default()
        },
        mcp_jsonrpc::SpawnOptions::default(),
    )
    .await
    .with_context(|| {
        if resolved_urls.sse_url == resolved_urls.post_url {
            format!(
                "connect streamable http mcp server (server={server_name} field={}) (url redacted)",
                resolved_urls.sse_url_field
            )
        } else {
            format!(
                "connect streamable http mcp server (server={server_name} fields={},{}) (urls redacted)",
                resolved_urls.sse_url_field, resolved_urls.post_url_field
            )
        }
    })?;

    Ok((client, None))
}

fn should_enforce_public_ip(ctx: &ConnectContext) -> bool {
    ctx.trust_mode != TrustMode::Trusted
        && !ctx
            .untrusted_streamable_http_policy
            .outbound
            .allow_private_ips
}

fn resolve_streamable_http_urls(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<ResolvedStreamableHttpUrls> {
    let (sse_url_raw, post_url_raw) = match (
        server_cfg.url(),
        server_cfg.sse_url(),
        server_cfg.http_url(),
    ) {
        (Some(url), None, None) => (url, url),
        (None, Some(sse_url), Some(http_url)) => (sse_url, http_url),
        _ => config_bail!(
            "mcp server {server_name}: set url or (sse_url + http_url) for transport=streamable_http"
        ),
    };

    let (sse_url_field, post_url_field) = if server_cfg.url().is_some() {
        ("url", "url")
    } else {
        ("sse_url", "http_url")
    };

    let sse_url = if ctx.trust_mode == TrustMode::Trusted {
        expand_placeholders_trusted(sse_url_raw, cwd).with_context(|| {
            format!(
                "expand url placeholder (server={server_name} field={sse_url_field}) (url redacted)"
            )
        })
        .map_err(|err| wrap_kind(ErrorKind::Config, err))?
    } else {
        sse_url_raw.to_string()
    };
    let post_url = if ctx.trust_mode == TrustMode::Trusted {
        expand_placeholders_trusted(post_url_raw, cwd).with_context(|| {
            format!(
                "expand url placeholder (server={server_name} field={post_url_field}) (url redacted)"
            )
        })
        .map_err(|err| wrap_kind(ErrorKind::Config, err))?
    } else {
        post_url_raw.to_string()
    };

    Ok(ResolvedStreamableHttpUrls {
        sse_url,
        post_url,
        sse_url_field,
        post_url_field,
    })
}

fn build_streamable_http_headers(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<HashMap<String, String>> {
    let capacity = server_cfg
        .http_headers()
        .len()
        .saturating_add(1)
        .saturating_add(usize::from(server_cfg.bearer_token_env_var().is_some()))
        .saturating_add(server_cfg.env_http_headers().len());
    let mut headers = HashMap::with_capacity(capacity);

    for (key, value) in server_cfg.http_headers() {
        if is_reserved_streamable_http_header(key) {
            config_bail!("mcp server {server_name}: http header is reserved by transport: {key}");
        }
        let value = if ctx.trust_mode == TrustMode::Trusted {
            expand_placeholders_trusted(value, cwd)
                .with_context(|| {
                    format!("expand http_header placeholder: {server_name} header={key}")
                })
                .map_err(|err| wrap_kind(ErrorKind::Config, err))?
        } else {
            value.to_string()
        };
        headers.insert(key.to_string(), value);
    }
    headers.insert(
        MCP_PROTOCOL_VERSION_HEADER.to_string(),
        ctx.protocol_version.clone(),
    );

    if let Some(env_var) = server_cfg.bearer_token_env_var() {
        debug_assert_eq!(ctx.trust_mode, TrustMode::Trusted);
        let token = std::env::var(env_var)
            .with_context(|| format!("read bearer token env var: {env_var}"))
            .map_err(|err| wrap_kind(ErrorKind::Config, err))?;
        headers.insert(AUTHORIZATION_HEADER.to_string(), format!("Bearer {token}"));
    }

    if !server_cfg.env_http_headers().is_empty() {
        debug_assert_eq!(ctx.trust_mode, TrustMode::Trusted);
        for (header, env_var) in server_cfg.env_http_headers().iter() {
            if is_reserved_streamable_http_env_header(header) {
                config_bail!(
                    "mcp server {server_name}: http header env var targets a reserved transport header: {header}"
                );
            }
            let value = std::env::var(env_var)
                .with_context(|| format!("read http header env var: {env_var}"))
                .map_err(|err| wrap_kind(ErrorKind::Config, err))?;
            headers.insert(header.to_string(), value);
        }
    }

    Ok(headers)
}

fn is_reserved_streamable_http_header(header: &str) -> bool {
    header.eq_ignore_ascii_case(MCP_PROTOCOL_VERSION_HEADER)
        || header.eq_ignore_ascii_case(AUTHORIZATION_HEADER)
        || header.eq_ignore_ascii_case(STREAMABLE_HTTP_SESSION_ID_HEADER)
}

fn is_reserved_streamable_http_env_header(header: &str) -> bool {
    is_reserved_streamable_http_header(header) || header.eq_ignore_ascii_case(AUTHORIZATION_HEADER)
}

fn resolve_unix_socket_path(unix_path: &Path, cwd: &Path) -> anyhow::Result<PathBuf> {
    let cwd = super::resolve_connection_cwd(cwd)?;
    super::path_identity::stable_path_identity(&absolutize_with_base(unix_path, &cwd))
        .map_err(Into::into)
}

pub(super) fn absolutize_with_base(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    base.join(path)
}

pub(super) fn stdout_log_path_within_root(
    stdout_log_path: &Path,
    root: &Path,
) -> std::io::Result<bool> {
    if !root.is_absolute() {
        return Ok(false);
    }

    let resolved_stdout_log_path = absolutize_with_base(stdout_log_path, root);
    let resolved_root = super::path_identity::stable_path_identity(root)?;
    let resolved_stdout_log_path =
        super::path_identity::stable_path_identity(&resolved_stdout_log_path)?;
    Ok(resolved_stdout_log_path.starts_with(&resolved_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MCP_PROTOCOL_VERSION;
    #[cfg(unix)]
    use std::path::PathBuf;

    fn trusted_connect_context() -> ConnectContext {
        ConnectContext {
            trust_mode: TrustMode::Trusted,
            untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy::default(),
            allow_stdout_log_outside_root: false,
            stdout_log_root: None,
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            request_timeout: Duration::from_secs(5),
        }
    }

    #[cfg(unix)]
    fn unix_socket_temp_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();

        if let Some(root) = std::env::var_os("OMNE_TEST_SHORT_TMPDIR") {
            let root = PathBuf::from(root);
            if !roots.iter().any(|candidate| candidate == &root) {
                roots.push(root);
            }
        }

        let temp_dir = std::env::temp_dir();
        if !roots.iter().any(|candidate| candidate == &temp_dir) {
            roots.push(temp_dir);
        }

        if std::env::var_os("TMPDIR").is_none()
            && std::env::temp_dir() == std::path::Path::new("/tmp")
        {
            let root = PathBuf::from("/var/tmp");
            if !roots.iter().any(|candidate| candidate == &root) {
                roots.push(root);
            }
        }

        roots
    }

    #[cfg(unix)]
    fn unique_socket_path(test_name: &str, label: &str) -> Option<PathBuf> {
        use std::os::unix::net::UnixListener;
        use std::time::{SystemTime, UNIX_EPOCH};

        let short_label: String = label
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .take(8)
            .collect();

        for root in unix_socket_temp_roots() {
            if !root.exists() && std::fs::create_dir_all(&root).is_err() {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&root) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }

            let Ok(tempdir) = tempfile::Builder::new()
                .prefix("of-ct-")
                .rand_bytes(3)
                .tempdir_in(&root)
            else {
                continue;
            };

            let path = tempdir.path().join(format!(
                "{short_label}-{}-{}.sock",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time after epoch")
                    .as_nanos()
            ));
            if let Ok(listener) = UnixListener::bind(&path) {
                drop(listener);
                let _ = std::fs::remove_file(&path);
                return Some(path);
            }
        }

        eprintln!(
            "skipping {test_name}: unable to create a short writable temp dir for unix socket test"
        );
        None
    }

    #[cfg(unix)]
    fn bind_unix_listener_or_skip(path: &std::path::Path) -> Option<tokio::net::UnixListener> {
        match tokio::net::UnixListener::bind(path) {
            Ok(listener) => Some(listener),
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping connect unix transport test: unix listener bind not permitted in this environment: {err}"
                );
                None
            }
            Err(err) => panic!("failed to bind unix listener at {}: {err}", path.display()),
        }
    }

    #[test]
    fn env_http_headers_cannot_override_authorization() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .env_http_headers_mut()
            .unwrap()
            .insert(AUTHORIZATION_HEADER.to_string(), "MCP_TOKEN".to_string());

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .expect_err("reserved Authorization env header should be rejected");
        assert!(
            err.to_string()
                .contains("http header env var targets a reserved transport header"),
            "{err:#}"
        );
    }

    #[test]
    fn http_headers_cannot_override_authorization() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg.http_headers_mut().unwrap().insert(
            AUTHORIZATION_HEADER.to_string(),
            "Bearer forged".to_string(),
        );

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .expect_err("reserved Authorization header should be rejected");
        assert!(
            err.to_string()
                .contains("http header is reserved by transport"),
            "{err:#}"
        );
    }

    #[test]
    fn http_headers_cannot_override_transport_owned_session_header() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .http_headers_mut()
            .unwrap()
            .insert("Mcp-Session-Id".to_string(), "forged-session".to_string());

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .expect_err("reserved session header should be rejected");
        assert!(
            err.to_string()
                .contains("http header is reserved by transport"),
            "{err:#}"
        );
    }

    #[test]
    fn env_http_headers_cannot_override_transport_owned_session_header() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .env_http_headers_mut()
            .unwrap()
            .insert("mcp-session-id".to_string(), "MCP_SESSION_ID".to_string());

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .expect_err("reserved session env header should be rejected");
        assert!(
            err.to_string()
                .contains("http header env var targets a reserved transport header"),
            "{err:#}"
        );
    }

    #[test]
    fn bearer_token_env_var_still_populates_authorization_header() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        let env_var = "PATH";
        server_cfg
            .set_bearer_token_env_var(Some(env_var.to_string()))
            .unwrap();
        let token = std::env::var(env_var).expect("PATH should be present in test environment");

        let headers = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .expect("bearer token env var should remain supported");
        let expected_authorization = format!("Bearer {token}");

        assert_eq!(
            headers.get(AUTHORIZATION_HEADER).map(String::as_str),
            Some(expected_authorization.as_str())
        );
        assert_eq!(
            headers.get(MCP_PROTOCOL_VERSION_HEADER).map(String::as_str),
            Some(MCP_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn effective_streamable_http_identity_resolves_env_backed_headers() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .set_bearer_token_env_var(Some("PATH".to_string()))
            .unwrap();
        server_cfg
            .env_http_headers_mut()
            .unwrap()
            .insert("x-env-header".to_string(), "PATH".to_string());

        let raw = raw_server_config_identity(&server_cfg);
        let effective = effective_server_config_identity(&ctx, "srv", &server_cfg, Path::new("."))
            .expect("resolve effective identity");

        assert_ne!(
            raw, effective,
            "config identity should capture resolved env-backed inputs instead of raw env var names"
        );
    }

    #[test]
    fn effective_unix_identity_resolves_relative_socket_path_against_connection_cwd() {
        let ctx = trusted_connect_context();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let cwd = tempdir.path().join("nested/run");
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let server_cfg = ServerConfig::unix(PathBuf::from("sock/service.sock")).expect("unix cfg");
        let effective =
            effective_server_config_identity(&ctx, "srv", &server_cfg, &cwd).expect("identity");

        assert_eq!(
            effective,
            ConnectionServerConfigIdentity::Unix {
                unix_path: cwd.join("sock/service.sock"),
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_unix_transport_resolves_relative_socket_path_against_connection_cwd() {
        let ctx = trusted_connect_context();
        let Some(socket_path) = unique_socket_path(
            "connect_unix_transport_resolves_relative_socket_path_against_connection_cwd",
            "connect",
        ) else {
            return;
        };
        let cwd = socket_path.parent().expect("socket parent").to_path_buf();
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };
        let accept_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept unix stream");
            drop(stream);
        });

        let socket_name = socket_path
            .file_name()
            .expect("socket file name")
            .to_string_lossy()
            .to_string();
        let server_cfg = ServerConfig::unix(PathBuf::from(socket_name)).expect("unix cfg");
        let (client, child) = connect_transport(&ctx, "srv", &server_cfg, &cwd)
            .await
            .expect("connect relative unix socket from connection cwd");

        drop(client);
        assert!(child.is_none(), "unix transport should not attach a child");
        accept_task.await.expect("listener task");
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[tokio::test]
    async fn stdio_stdout_log_root_uses_config_root_not_request_cwd() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let root = tempdir.path().join("workspace");
        let cwd = root.join("subdir");
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let mut server_cfg = ServerConfig::stdio(vec![
            "sh".to_string(),
            "-c".to_string(),
            "exec cat".to_string(),
        ])
        .expect("stdio config");
        server_cfg
            .set_stdout_log(Some(crate::StdoutLogConfig {
                path: root.join("logs/server.stdout.log"),
                max_bytes_per_part: 1024,
                max_parts: Some(1),
            }))
            .expect("stdout log config");

        let ctx = ConnectContext {
            trust_mode: TrustMode::Trusted,
            untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy::default(),
            allow_stdout_log_outside_root: false,
            stdout_log_root: Some(root.clone()),
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            request_timeout: Duration::from_secs(1),
        };

        let (client, child) = connect_transport(&ctx, "srv", &server_cfg, &cwd)
            .await
            .expect("config-root anchored stdout_log path should be accepted");
        drop(client);
        let mut child = child.expect("stdio child");
        child.kill().await.expect("kill child");
        let _ = child.wait().await;
    }

    #[test]
    fn private_ip_override_disables_public_ip_pinning_for_streamable_http() {
        let mut ctx = trusted_connect_context();
        assert!(!should_enforce_public_ip(&ctx));

        ctx.trust_mode = TrustMode::Untrusted;
        assert!(should_enforce_public_ip(&ctx));

        ctx.untrusted_streamable_http_policy
            .outbound
            .allow_private_ips = true;
        assert!(!should_enforce_public_ip(&ctx));
    }
}
