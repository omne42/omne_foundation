use std::net::IpAddr;

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

    for header in server_cfg.http_headers_required().keys() {
        if is_untrusted_sensitive_http_header(header) {
            anyhow::bail!(
                "refusing to send sensitive http header in untrusted mode: {server_name} header={header} (set Manager::with_trust_mode(TrustMode::Trusted) to override)"
            );
        }
    }

    if !policy.allow_custom_headers && !server_cfg.http_headers_required().is_empty() {
        anyhow::bail!(
            "refusing to send custom http headers in untrusted mode: {server_name} (set Manager::with_untrusted_streamable_http_policy(...) with allow_custom_headers=true or use TrustMode::Trusted to override)"
        );
    }

    validate_streamable_http_url_untrusted(policy, server_name, url_field, url)?;

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
        .map_err(|err| map_untrusted_outbound_url_error(err, server_name, url_field))?;

    if !policy.allow_public_hosts
        && policy.outbound.allowed_hosts.is_empty()
        && targets_public_host_without_explicit_untrusted_opt_in(policy, &parsed)?
    {
        anyhow::bail!(
            "refusing to connect arbitrary public streamable http host in untrusted mode: {server_name} field={url_field} (set UntrustedStreamableHttpPolicy::allow_public_hosts=true, configure outbound.allowed_hosts, or use TrustMode::Trusted to override)"
        );
    }

    Ok(())
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

fn targets_public_host_without_explicit_untrusted_opt_in(
    policy: &UntrustedStreamableHttpPolicy,
    url: &reqwest::Url,
) -> anyhow::Result<bool> {
    let host = url
        .host_str()
        .map(|host| host.trim_end_matches('.'))
        .ok_or_else(|| anyhow::anyhow!("streamable http url must include a host"))?;
    let host_for_ip = host.trim_start_matches('[').trim_end_matches(']');

    if is_local_or_single_label_host(host, host_for_ip) {
        return Ok(false);
    }

    if host_for_ip.parse::<IpAddr>().is_ok() {
        if !policy.outbound.allow_private_ips {
            return Ok(true);
        }

        let mut public_only_policy = policy.outbound.clone();
        public_only_policy.allow_private_ips = false;
        return match http_kit::validate_untrusted_outbound_url(&public_only_policy, url) {
            Ok(()) => Ok(true),
            Err(http_kit::UntrustedOutboundError::NonGlobalIpNotAllowed { .. }) => Ok(false),
            Err(err) => Err(anyhow::anyhow!(
                "classify untrusted streamable http host after validation: {err}"
            )),
        };
    }

    Ok(true)
}

fn is_loopback_hostname(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("localhost.localdomain")
        || ends_with_ignore_ascii_case(host, ".localhost")
}

fn ends_with_ignore_ascii_case(haystack: &str, suffix: &str) -> bool {
    if suffix.len() > haystack.len() {
        return false;
    }
    haystack
        .get(haystack.len() - suffix.len()..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

fn is_local_or_single_label_host(host: &str, host_for_ip: &str) -> bool {
    let is_ip_literal = host_for_ip.parse::<IpAddr>().is_ok();
    let is_single_label = !is_ip_literal && !host.contains('.');
    is_loopback_hostname(host)
        || ends_with_ignore_ascii_case(host, ".local")
        || ends_with_ignore_ascii_case(host, ".localdomain")
        || is_single_label
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
