use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
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
        if !ctx.allow_stdout_log_outside_root
            && !stdout_log_path_within_root(&resolved_log_path, &cwd)
        {
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
) -> anyhow::Result<(mcp_jsonrpc::Client, Option<Child>)> {
    if ctx.trust_mode == TrustMode::Untrusted {
        config_bail!(
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
            enforce_public_ip: ctx.trust_mode != TrustMode::Trusted,
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
}

fn is_reserved_streamable_http_env_header(header: &str) -> bool {
    is_reserved_streamable_http_header(header) || header.eq_ignore_ascii_case(AUTHORIZATION_HEADER)
}

pub(super) fn absolutize_with_base(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    base.join(path)
}

fn normalize_path_for_prefix_check(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn canonicalize_existing_prefix(path: &Path) -> Option<PathBuf> {
    let normalized = normalize_path_for_prefix_check(path);
    let mut existing = normalized.as_path();
    let mut missing_components = Vec::new();

    while std::fs::symlink_metadata(existing).is_err() {
        let component = existing.file_name()?;
        missing_components.push(component.to_os_string());
        existing = existing.parent()?;
    }

    let mut resolved = std::fs::canonicalize(existing).ok()?;
    for component in missing_components.iter().rev() {
        resolved.push(component);
    }
    Some(resolved)
}

pub(super) fn stdout_log_path_within_root(stdout_log_path: &Path, root: &Path) -> bool {
    if !root.is_absolute() {
        return false;
    }

    let resolved_stdout_log_path = absolutize_with_base(stdout_log_path, root);
    let Some(resolved_root) = canonicalize_existing_prefix(root) else {
        return false;
    };
    let Some(resolved_stdout_log_path) = canonicalize_existing_prefix(&resolved_stdout_log_path)
    else {
        return false;
    };
    resolved_stdout_log_path.starts_with(&resolved_root)
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
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            request_timeout: Duration::from_secs(5),
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
}
