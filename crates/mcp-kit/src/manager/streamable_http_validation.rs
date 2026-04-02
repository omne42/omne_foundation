use anyhow::Context;

use crate::{ServerConfig, TrustMode, UntrustedStreamableHttpPolicy};

pub(super) fn validate_streamable_http_config(
    trust_mode: TrustMode,
    policy: &UntrustedStreamableHttpPolicy,
    server_name: &str,
    url_field: &str,
    url: &str,
    server_cfg: &ServerConfig,
) -> anyhow::Result<()> {
    if trust_mode == TrustMode::Trusted {
        return Ok(());
    }

    validate_streamable_http_url_untrusted(policy, server_name, url_field, url)?;

    for header in server_cfg.http_headers_required().keys() {
        if is_untrusted_sensitive_http_header(header) {
            anyhow::bail!(
                "refusing to send sensitive http header in untrusted mode: {server_name} header={header} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }
    }

    Ok(())
}

pub(super) fn validate_streamable_http_url_untrusted(
    policy: &UntrustedStreamableHttpPolicy,
    server_name: &str,
    url_field: &str,
    url: &str,
) -> anyhow::Result<()> {
    let parsed = parse_streamable_http_url(server_name, url_field, url)?;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!(
            "refusing to use url credentials in untrusted mode: {server_name} field={url_field} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        );
    }

    match parsed.scheme() {
        "https" => {}
        "http" if !policy.require_https => {}
        _ => {
            anyhow::bail!(
                "refusing to connect non-https streamable http url in untrusted mode: {server_name} field={url_field} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }
    }

    http_kit::validate_untrusted_outbound_url(&policy.outbound, &parsed)
        .map_err(|err| map_untrusted_outbound_url_error(err, server_name, url_field))
}

pub(super) async fn validate_streamable_http_url_untrusted_dns(
    policy: &UntrustedStreamableHttpPolicy,
    server_name: &str,
    url_field: &str,
    url: &str,
) -> anyhow::Result<()> {
    let parsed = parse_streamable_http_url(server_name, url_field, url)?;
    http_kit::validate_untrusted_outbound_url_dns(&policy.outbound, &parsed)
        .await
        .map_err(|err| map_untrusted_outbound_dns_error(err, server_name, url_field))
}

fn parse_streamable_http_url(
    server_name: &str,
    url_field: &str,
    url: &str,
) -> anyhow::Result<reqwest::Url> {
    reqwest::Url::parse(url).with_context(|| {
        format!(
            "invalid streamable http url (server={server_name} field={url_field}) (url redacted)"
        )
    })
}

fn is_untrusted_sensitive_http_header(header: &str) -> bool {
    let header = header.trim();
    header.eq_ignore_ascii_case("authorization")
        || header.eq_ignore_ascii_case("proxy-authorization")
        || header.eq_ignore_ascii_case("cookie")
}

fn map_untrusted_outbound_url_error(
    err: http_kit::UntrustedOutboundError,
    server_name: &str,
    url_field: &str,
) -> anyhow::Error {
    match err {
        http_kit::UntrustedOutboundError::MissingHost => anyhow::anyhow!(
            "streamable http url must include a host (server={server_name} field={url_field}) (url redacted)"
        ),
        http_kit::UntrustedOutboundError::LocalhostHostNotAllowed { host } => anyhow::anyhow!(
            "refusing to connect localhost/local/single-label domain in untrusted mode: {server_name} host={host} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        ),
        http_kit::UntrustedOutboundError::HostNotAllowed { host } => anyhow::anyhow!(
            "refusing to connect streamable http host not in allowlist in untrusted mode: {server_name} host={host} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        ),
        http_kit::UntrustedOutboundError::NonGlobalIpNotAllowed { host } => anyhow::anyhow!(
            "refusing to connect non-global ip in untrusted mode: {server_name} host={host} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
        ),
        http_kit::UntrustedOutboundError::DnsLookupFailed { host, message } => anyhow::anyhow!(
            "refusing to connect hostname with failed dns lookup in untrusted mode: {server_name} host={host} err={message}"
        ),
        http_kit::UntrustedOutboundError::DnsLookupTimedOut { host } => anyhow::anyhow!(
            "refusing to connect hostname with timed out dns lookup in untrusted mode: {server_name} host={host}"
        ),
        http_kit::UntrustedOutboundError::ResolvedToNonGlobalIp { host, ip } => anyhow::anyhow!(
            "refusing to connect hostname that resolves to non-global ip in untrusted mode: {server_name} host={host} ip={ip} (set Manager::with_trust_mode(TrustMode::Trusted) or allow_private_ips to override)"
        ),
        http_kit::UntrustedOutboundError::MissingPortOrKnownDefault => anyhow::anyhow!(
            "streamable http url must include a port or known scheme (server={server_name} field={url_field}) (url redacted)"
        ),
    }
}

fn map_untrusted_outbound_dns_error(
    err: http_kit::UntrustedOutboundError,
    server_name: &str,
    url_field: &str,
) -> anyhow::Error {
    match err {
        http_kit::UntrustedOutboundError::MissingHost => anyhow::anyhow!(
            "streamable http url must include a host (server={server_name} field={url_field}) (url redacted)"
        ),
        http_kit::UntrustedOutboundError::MissingPortOrKnownDefault => anyhow::anyhow!(
            "streamable http url must include a port or known scheme (server={server_name} field={url_field}) (url redacted)"
        ),
        http_kit::UntrustedOutboundError::DnsLookupFailed { host, message } => anyhow::anyhow!(
            "refusing to connect hostname with failed dns lookup in untrusted mode: {server_name} host={host} err={message}"
        ),
        http_kit::UntrustedOutboundError::DnsLookupTimedOut { host } => anyhow::anyhow!(
            "refusing to connect hostname with timed out dns lookup in untrusted mode: {server_name} host={host}"
        ),
        http_kit::UntrustedOutboundError::ResolvedToNonGlobalIp { host, ip } => anyhow::anyhow!(
            "refusing to connect hostname that resolves to non-global ip in untrusted mode: {server_name} host={host} ip={ip} (set Manager::with_trust_mode(TrustMode::Trusted) or allow_private_ips to override)"
        ),
        other => map_untrusted_outbound_url_error(other, server_name, url_field),
    }
}

pub(crate) fn should_disconnect_after_jsonrpc_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<mcp_jsonrpc::Error>()
            .is_some_and(|err| {
                matches!(err, mcp_jsonrpc::Error::Io(_))
                    || matches!(
                        err,
                        mcp_jsonrpc::Error::Protocol(protocol_err)
                            if protocol_err.kind != mcp_jsonrpc::ProtocolErrorKind::WaitTimeout
                    )
            })
    })
}
