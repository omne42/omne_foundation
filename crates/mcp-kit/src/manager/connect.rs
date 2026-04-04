use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use secret_kit::runtime::{SecretCommandRuntime, SecretEnvironment};
use secret_kit::{SecretString, spec};
use tokio::process::{Child, Command};

use crate::protocol::{
    AUTHORIZATION_HEADER, MCP_PROTOCOL_VERSION_HEADER, is_reserved_streamable_http_transport_header,
};
use crate::{ServerConfig, Transport, TrustMode, UntrustedStreamableHttpPolicy};

use super::path_identity::{canonicalize_existing_prefix, normalize_path_lexically};
use super::placeholders::{
    apply_stdio_baseline_env, expand_placeholders_trusted_os, expand_root_placeholders_trusted,
};
use super::streamable_http_validation::{
    validate_streamable_http_config, validate_streamable_http_url_untrusted_dns,
};

#[derive(Debug, Clone)]
pub(crate) struct ConnectContext {
    pub(crate) trust_mode: TrustMode,
    pub(crate) untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy,
    pub(crate) allow_stdout_log_outside_root: bool,
    pub(crate) stdout_log_root: PathBuf,
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

#[derive(Debug, Default, Clone, Copy)]
struct AmbientStreamableHttpSecretContext;

impl SecretEnvironment for AmbientStreamableHttpSecretContext {
    fn get_secret(&self, key: &str) -> Option<SecretString> {
        std::env::var(key).ok().map(SecretString::new)
    }
}

impl SecretCommandRuntime for AmbientStreamableHttpSecretContext {}

static AMBIENT_STREAMABLE_HTTP_SECRET_CONTEXT: AmbientStreamableHttpSecretContext =
    AmbientStreamableHttpSecretContext;

pub(super) fn should_enforce_streamable_http_public_ip_pinning(
    trust_mode: TrustMode,
    policy: &UntrustedStreamableHttpPolicy,
    sse_url: &str,
    post_url: &str,
) -> bool {
    if trust_mode == TrustMode::Trusted {
        return false;
    }

    if policy.outbound.allow_private_ips {
        return false;
    }

    if !policy.outbound.allow_localhost {
        return true;
    }

    !streamable_http_urls_include_loopback_hostname(sse_url, post_url)
}

fn streamable_http_urls_include_loopback_hostname(sse_url: &str, post_url: &str) -> bool {
    streamable_http_url_uses_loopback_hostname(sse_url)
        || streamable_http_url_uses_loopback_hostname(post_url)
}

fn streamable_http_url_uses_loopback_hostname(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_end_matches('.').to_string())
        })
        .is_some_and(|host| is_loopback_hostname(&host))
}

fn is_loopback_hostname(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("localhost.localdomain")
        || host
            .get(host.len().saturating_sub(".localhost".len())..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".localhost"))
}

pub(crate) async fn connect_transport(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    match server_cfg.transport() {
        Transport::Stdio => connect_stdio_transport(ctx, server_name, server_cfg, cwd).await,
        Transport::Unix => connect_unix_transport(ctx, server_name, server_cfg).await,
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
        anyhow::bail!(
            "refusing to spawn mcp server in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        );
    }

    let expanded_argv = server_cfg
        .argv_required()
        .iter()
        .enumerate()
        .map(|(idx, arg)| {
            expand_placeholders_trusted_os(arg, &cwd).with_context(|| {
                format!("expand argv placeholder (server={server_name} argv[{idx}] redacted)")
            })
        })
        .collect::<anyhow::Result<Vec<std::ffi::OsString>>>()?;

    let mut cmd = Command::new(&expanded_argv[0]);
    cmd.args(expanded_argv.iter().skip(1));
    cmd.current_dir(&cwd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    // Library callers own stderr routing. Default to /dev/null instead of leaking server logs or
    // secrets into the host process boundary.
    cmd.stderr(Stdio::null());
    if !server_cfg.inherit_env_required() {
        cmd.env_clear();
        apply_stdio_baseline_env(&mut cmd);
    }
    for (key, value) in server_cfg.env_required().iter() {
        let value = expand_placeholders_trusted_os(value, &cwd)
            .with_context(|| format!("expand env placeholder: {key}"))?;
        cmd.env(key, value);
    }
    cmd.kill_on_drop(true);

    let stdout_log = server_cfg.stdout_log().map(|log| {
        let resolved_log_path = absolutize_with_base(&log.path, &cwd);
        if !ctx.allow_stdout_log_outside_root
            && !stdout_log_path_within_root(&resolved_log_path, &ctx.stdout_log_root)
                .with_context(|| {
                format!(
                    "check stdout_log.path root boundary for server {server_name}: {}",
                    log.path.display()
                )
            })?
        {
            anyhow::bail!(
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
            server_cfg.argv_required().len()
        )
    })?;
    let child = client.take_child();
    Ok((client, child))
}

async fn connect_unix_transport(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    if ctx.trust_mode == TrustMode::Untrusted {
        anyhow::bail!(
            "refusing to connect unix mcp server in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        );
    }
    let unix_path = server_cfg.unix_path_required();
    let client = mcp_jsonrpc::Client::connect_unix(unix_path)
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

    if ctx.trust_mode != TrustMode::Trusted {
        if server_cfg.bearer_token_secret().is_some() {
            anyhow::bail!(
                "refusing to resolve bearer token secret in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }
        if server_cfg
            .secret_http_headers()
            .is_some_and(|headers| !headers.is_empty())
        {
            anyhow::bail!(
                "refusing to resolve secret-backed http headers in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }
    }

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

    let headers = build_streamable_http_headers(ctx, server_name, server_cfg, cwd).await?;
    let enforce_public_ip = should_enforce_streamable_http_public_ip_pinning(
        ctx.trust_mode,
        &ctx.untrusted_streamable_http_policy,
        &resolved_urls.sse_url,
        &resolved_urls.post_url,
    );
    let client = mcp_jsonrpc::Client::connect_streamable_http_split_with_options(
        &resolved_urls.sse_url,
        &resolved_urls.post_url,
        mcp_jsonrpc::StreamableHttpOptions {
            headers,
            // `mcp-jsonrpc` only exposes a strict public-only DNS pinning mode. Once the
            // untrusted policy intentionally allows non-public endpoints, keep syntax/DNS
            // validation but stop forcing the transport into a contradictory public-only socket
            // selection path.
            enforce_public_ip,
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
        _ => {
            anyhow::bail!(
                "mcp server {server_name}: set url or (sse_url + http_url) for transport=streamable_http"
            )
        }
    };

    let (sse_url_field, post_url_field) = if server_cfg.url().is_some() {
        ("url", "url")
    } else {
        ("sse_url", "http_url")
    };

    let sse_url = if ctx.trust_mode == TrustMode::Trusted {
        expand_root_placeholders_trusted(sse_url_raw, cwd).with_context(|| {
            format!(
                "expand url placeholder (server={server_name} field={sse_url_field}) (url redacted)"
            )
        })?
    } else {
        sse_url_raw.to_string()
    };
    let post_url = if ctx.trust_mode == TrustMode::Trusted {
        expand_root_placeholders_trusted(post_url_raw, cwd).with_context(|| {
            format!(
                "expand url placeholder (server={server_name} field={post_url_field}) (url redacted)"
            )
        })?
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

async fn build_streamable_http_headers(
    ctx: &ConnectContext,
    server_name: &str,
    server_cfg: &ServerConfig,
    cwd: &Path,
) -> anyhow::Result<HashMap<String, String>> {
    let capacity = server_cfg
        .http_headers_required()
        .len()
        .saturating_add(1)
        .saturating_add(usize::from(server_cfg.bearer_token_secret().is_some()))
        .saturating_add(server_cfg.secret_http_headers_required().len());
    let mut headers = HashMap::with_capacity(capacity);
    let mut seen_names = HashSet::with_capacity(capacity);

    for (key, value) in server_cfg.http_headers_required() {
        if is_reserved_streamable_http_header(key) {
            anyhow::bail!("mcp server {server_name}: http header is reserved by transport: {key}");
        }
        let value = if ctx.trust_mode == TrustMode::Trusted {
            expand_root_placeholders_trusted(value, cwd).with_context(|| {
                format!("expand http_header placeholder: {server_name} header={key}")
            })?
        } else {
            value.to_string()
        };
        insert_streamable_http_header(&mut headers, &mut seen_names, server_name, key, value)?;
    }
    insert_streamable_http_header(
        &mut headers,
        &mut seen_names,
        server_name,
        MCP_PROTOCOL_VERSION_HEADER,
        ctx.protocol_version.clone(),
    )?;

    if let Some(secret_spec) = server_cfg.bearer_token_secret() {
        debug_assert_eq!(ctx.trust_mode, TrustMode::Trusted);
        let token = spec::resolve_secret(secret_spec, &AMBIENT_STREAMABLE_HTTP_SECRET_CONTEXT)
            .await
            .with_context(|| {
                format!("resolve bearer token secret (server={server_name}) (spec redacted)")
            })?;
        insert_streamable_http_header(
            &mut headers,
            &mut seen_names,
            server_name,
            AUTHORIZATION_HEADER,
            format!("Bearer {}", token.expose_secret()),
        )?;
    }

    if !server_cfg.secret_http_headers_required().is_empty() {
        debug_assert_eq!(ctx.trust_mode, TrustMode::Trusted);
        for (header, secret_spec) in server_cfg.secret_http_headers_required().iter() {
            if is_reserved_streamable_http_env_header(header) {
                anyhow::bail!(
                    "mcp server {server_name}: secret-backed http header targets a reserved transport header: {header}"
                );
            }
            let value = spec::resolve_secret(secret_spec, &AMBIENT_STREAMABLE_HTTP_SECRET_CONTEXT)
                .await
                .with_context(|| {
                    format!(
                        "resolve secret-backed http header: {server_name} header={header} (spec redacted)"
                    )
                })?;
            insert_streamable_http_header(
                &mut headers,
                &mut seen_names,
                server_name,
                header,
                value.expose_secret().to_owned(),
            )?;
        }
    }

    Ok(headers)
}

fn insert_streamable_http_header(
    headers: &mut HashMap<String, String>,
    seen_names: &mut HashSet<reqwest::header::HeaderName>,
    server_name: &str,
    key: &str,
    value: String,
) -> anyhow::Result<()> {
    let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
        .with_context(|| format!("mcp server {server_name}: invalid http header name: {key}"))?;
    if !seen_names.insert(name) {
        anyhow::bail!("mcp server {server_name}: duplicate http header name ignoring case: {key}");
    }
    headers.insert(key.to_string(), value);
    Ok(())
}

fn is_reserved_streamable_http_header(header: &str) -> bool {
    is_reserved_streamable_http_transport_header(header)
}

fn is_reserved_streamable_http_env_header(header: &str) -> bool {
    is_reserved_streamable_http_header(header)
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
) -> anyhow::Result<bool> {
    if !root.is_absolute() {
        return Ok(false);
    }

    let resolved_stdout_log_path = absolutize_with_base(stdout_log_path, root);
    let normalized_root = normalize_path_lexically(root);
    let normalized_stdout_log_path = normalize_path_lexically(&resolved_stdout_log_path);
    let Some(resolved_root) =
        canonicalize_existing_prefix(&normalized_root, "root-boundary check")?
    else {
        return Ok(false);
    };
    let Some(resolved_stdout_log_path) =
        canonicalize_existing_prefix(&normalized_stdout_log_path, "root-boundary check")?
    else {
        return Ok(false);
    };
    Ok(resolved_stdout_log_path.starts_with(&resolved_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MCP_PROTOCOL_VERSION;

    fn trusted_connect_context() -> ConnectContext {
        ConnectContext {
            trust_mode: TrustMode::Trusted,
            untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy::default(),
            allow_stdout_log_outside_root: false,
            stdout_log_root: PathBuf::from("/"),
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            request_timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn secret_http_headers_cannot_override_authorization() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg.secret_http_headers_mut().unwrap().insert(
            AUTHORIZATION_HEADER.to_string(),
            "secret://env/MCP_TOKEN".to_string(),
        );

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .await
            .expect_err("reserved Authorization env header should be rejected");
        assert!(
            err.to_string()
                .contains("secret-backed http header targets a reserved transport header"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn bearer_token_secret_still_populates_authorization_header() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        let env_var = "PATH";
        server_cfg
            .set_bearer_token_secret(Some(format!("secret://env/{env_var}")))
            .unwrap();
        let token = std::env::var(env_var).expect("PATH should be present in test environment");

        let headers = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .await
            .expect("bearer token secret should remain supported");
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

    #[tokio::test]
    async fn static_http_headers_reject_env_placeholders() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .http_headers_mut()
            .unwrap()
            .insert("X-Test".to_string(), "${PATH}".to_string());

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .await
            .expect_err("transport http headers should reject env placeholders");
        assert!(
            format!("{err:#}").contains("placeholder `PATH` is not allowed"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn http_headers_cannot_override_transport_owned_headers() {
        let ctx = trusted_connect_context();
        for header in ["Accept", "Content-Type", "mcp-session-id"] {
            let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
            server_cfg
                .http_headers_mut()
                .unwrap()
                .insert(header.to_string(), "override".to_string());

            let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
                .await
                .expect_err("transport-owned static header should be rejected");
            assert!(
                err.to_string()
                    .contains("http header is reserved by transport"),
                "header={header} err={err:#}"
            );
        }
    }

    #[tokio::test]
    async fn secret_http_headers_cannot_override_transport_owned_headers() {
        let ctx = trusted_connect_context();
        for header in ["Accept", "Content-Type", "mcp-session-id"] {
            let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
            server_cfg
                .secret_http_headers_mut()
                .unwrap()
                .insert(header.to_string(), "secret://env/MCP_TOKEN".to_string());

            let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
                .await
                .expect_err("transport-owned env header should be rejected");
            assert!(
                err.to_string()
                    .contains("secret-backed http header targets a reserved transport header"),
                "header={header} err={err:#}"
            );
        }
    }

    #[tokio::test]
    async fn streamable_http_headers_reject_duplicate_names_ignoring_case() {
        let ctx = trusted_connect_context();
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .http_headers_mut()
            .unwrap()
            .insert("X-Test".to_string(), "static".to_string());
        server_cfg
            .secret_http_headers_mut()
            .unwrap()
            .insert("x-test".to_string(), "secret://env/PATH".to_string());

        let err = build_streamable_http_headers(&ctx, "srv", &server_cfg, Path::new("."))
            .await
            .expect_err("case-insensitive duplicate headers should be rejected");
        assert!(
            err.to_string()
                .contains("duplicate http header name ignoring case"),
            "{err:#}"
        );
    }
}
