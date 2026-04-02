use std::net::IpAddr;
use std::time::Duration;

use thiserror::Error;

use crate::ip::{is_always_disallowed_ip, is_non_global_ip, normalize_ip};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedOutboundPolicy {
    pub allow_localhost: bool,
    pub allow_private_ips: bool,
    pub dns_check: bool,
    pub dns_timeout: Duration,
    pub dns_fail_open: bool,
    pub allowed_hosts: Vec<String>,
}

impl Default for UntrustedOutboundPolicy {
    fn default() -> Self {
        Self {
            allow_localhost: false,
            allow_private_ips: false,
            dns_check: true,
            dns_timeout: Duration::from_secs(2),
            dns_fail_open: false,
            allowed_hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UntrustedOutboundError {
    #[error("url must have a host")]
    MissingHost,
    #[error("url must include a port or known default scheme")]
    MissingPortOrKnownDefault,
    #[error("localhost/local/single-label host is not allowed: {host}")]
    LocalhostHostNotAllowed { host: String },
    #[error("url host is not in allowlist: {host}")]
    HostNotAllowed { host: String },
    #[error("non-global ip is not allowed: host={host}")]
    NonGlobalIpNotAllowed { host: String },
    #[error("dns lookup failed for host {host}: {message}")]
    DnsLookupFailed { host: String, message: String },
    #[error("dns lookup timed out for host {host}")]
    DnsLookupTimedOut { host: String },
    #[error("hostname resolves to non-global ip: host={host} ip={ip}")]
    ResolvedToNonGlobalIp { host: String, ip: IpAddr },
}

pub fn validate_untrusted_outbound_url(
    policy: &UntrustedOutboundPolicy,
    url: &reqwest::Url,
) -> Result<(), UntrustedOutboundError> {
    let host = normalized_host(url)?;
    let host_for_ip = host_for_ip_literal(host);

    if is_local_or_single_label_host(host, host_for_ip)
        && !(policy.allow_localhost && is_loopback_hostname(host))
    {
        return Err(UntrustedOutboundError::LocalhostHostNotAllowed {
            host: host.to_string(),
        });
    }

    if !policy.allowed_hosts.is_empty()
        && !policy
            .allowed_hosts
            .iter()
            .any(|allowed| host_matches_allowlist(host, allowed))
    {
        return Err(UntrustedOutboundError::HostNotAllowed {
            host: host.to_string(),
        });
    }

    if let Ok(ip) = host_for_ip.parse::<IpAddr>() {
        if is_ip_disallowed_for_host(policy, host, ip) {
            return Err(UntrustedOutboundError::NonGlobalIpNotAllowed {
                host: host.to_string(),
            });
        }
    }

    Ok(())
}

pub async fn validate_untrusted_outbound_url_dns(
    policy: &UntrustedOutboundPolicy,
    url: &reqwest::Url,
) -> Result<(), UntrustedOutboundError> {
    if !policy.dns_check {
        return Ok(());
    }

    let host = normalized_host(url)?;
    let host_for_ip = host_for_ip_literal(host);
    if host_for_ip.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    let port = url
        .port_or_known_default()
        .ok_or(UntrustedOutboundError::MissingPortOrKnownDefault)?;

    let addrs = match tokio::time::timeout(
        policy.dns_timeout,
        tokio::net::lookup_host((host_for_ip, port)),
    )
    .await
    {
        Ok(Ok(addrs)) => addrs,
        Ok(Err(err)) => {
            if policy.dns_fail_open {
                return Ok(());
            }
            return Err(UntrustedOutboundError::DnsLookupFailed {
                host: host.to_string(),
                message: err.to_string(),
            });
        }
        Err(_) => {
            if policy.dns_fail_open {
                return Ok(());
            }
            return Err(UntrustedOutboundError::DnsLookupTimedOut {
                host: host.to_string(),
            });
        }
    };

    validate_resolved_addrs(policy, host, addrs)
}

fn validate_resolved_addrs(
    policy: &UntrustedOutboundPolicy,
    host: &str,
    addrs: impl IntoIterator<Item = std::net::SocketAddr>,
) -> Result<(), UntrustedOutboundError> {
    for addr in addrs {
        let ip = normalize_ip(addr.ip());
        if is_ip_disallowed_for_host(policy, host, ip) {
            return Err(UntrustedOutboundError::ResolvedToNonGlobalIp {
                host: host.to_string(),
                ip,
            });
        }
    }

    Ok(())
}

fn is_ip_disallowed_for_host(policy: &UntrustedOutboundPolicy, host: &str, ip: IpAddr) -> bool {
    let _ = host;

    if is_localhost_resolution_ip(ip) {
        return true;
    }

    if is_always_disallowed_ip(ip) {
        return true;
    }

    let ip = normalize_ip(ip);
    if is_private_ip(ip) {
        return !policy.allow_private_ips;
    }

    is_non_global_ip(ip)
}

fn is_private_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(ip) => ip.is_unique_local(),
    }
}

fn is_localhost_resolution_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(ip) => ip.is_loopback() || is_host_local_ipv4(ip),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

fn is_host_local_ipv4(ip: std::net::Ipv4Addr) -> bool {
    ip.octets()[0] == 0 && !ip.is_unspecified()
}

fn normalized_host(url: &reqwest::Url) -> Result<&str, UntrustedOutboundError> {
    url.host_str()
        .map(|host| host.trim_end_matches('.'))
        .ok_or(UntrustedOutboundError::MissingHost)
}

fn host_for_ip_literal(host: &str) -> &str {
    host.trim_start_matches('[').trim_end_matches(']')
}

fn is_loopback_hostname(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("localhost.localdomain")
        || ends_with_ignore_ascii_case(host, ".localhost")
}

fn is_local_or_single_label_host(host: &str, host_for_ip: &str) -> bool {
    let is_ip_literal = host_for_ip.parse::<IpAddr>().is_ok();
    let is_single_label = !is_ip_literal && !host.contains('.');
    is_loopback_hostname(host)
        || ends_with_ignore_ascii_case(host, ".local")
        || ends_with_ignore_ascii_case(host, ".localdomain")
        || is_single_label
}

fn ends_with_ignore_ascii_case(haystack: &str, suffix: &str) -> bool {
    if suffix.len() > haystack.len() {
        return false;
    }
    haystack
        .get(haystack.len() - suffix.len()..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

fn host_matches_allowlist(host: &str, allowed: &str) -> bool {
    let host = host.trim().trim_end_matches('.');
    let allowed = allowed.trim().trim_end_matches('.');
    if allowed.is_empty() {
        return false;
    }
    let host_ip = host_for_ip_literal(host)
        .parse::<IpAddr>()
        .ok()
        .map(normalize_ip);
    let allowed_ip = host_for_ip_literal(allowed)
        .parse::<IpAddr>()
        .ok()
        .map(normalize_ip);
    match (host_ip, allowed_ip) {
        (Some(host_ip), Some(allowed_ip)) => return host_ip == allowed_ip,
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    if host.eq_ignore_ascii_case(allowed) {
        return true;
    }
    if host.len() <= allowed.len() + 1 {
        return false;
    }
    if !ends_with_ignore_ascii_case(host, allowed) {
        return false;
    }
    let boundary = host.len() - allowed.len() - 1;
    host.as_bytes().get(boundary).is_some_and(|ch| *ch == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_single_label_hosts_by_default() {
        let policy = UntrustedOutboundPolicy::default();
        let url = reqwest::Url::parse("https://internal/mcp").expect("parse url");
        let err = validate_untrusted_outbound_url(&policy, &url).expect_err("expected rejection");
        assert!(matches!(
            err,
            UntrustedOutboundError::LocalhostHostNotAllowed { .. }
        ));
    }

    #[test]
    fn allow_localhost_only_allows_loopback_hostnames() {
        let policy = UntrustedOutboundPolicy {
            allow_localhost: true,
            ..Default::default()
        };

        let localhost = reqwest::Url::parse("https://localhost/mcp").expect("parse url");
        validate_untrusted_outbound_url(&policy, &localhost).expect("localhost should be allowed");

        let localhost_subdomain =
            reqwest::Url::parse("https://demo.localhost/mcp").expect("parse url");
        validate_untrusted_outbound_url(&policy, &localhost_subdomain)
            .expect("*.localhost should be allowed");

        let local_domain = reqwest::Url::parse("https://service.local/mcp").expect("parse url");
        let err =
            validate_untrusted_outbound_url(&policy, &local_domain).expect_err("*.local blocks");
        assert!(matches!(
            err,
            UntrustedOutboundError::LocalhostHostNotAllowed { .. }
        ));

        let localdomain =
            reqwest::Url::parse("https://service.localdomain/mcp").expect("parse url");
        let err = validate_untrusted_outbound_url(&policy, &localdomain)
            .expect_err("*.localdomain blocks");
        assert!(matches!(
            err,
            UntrustedOutboundError::LocalhostHostNotAllowed { .. }
        ));

        let single_label = reqwest::Url::parse("https://internal/mcp").expect("parse url");
        let err = validate_untrusted_outbound_url(&policy, &single_label)
            .expect_err("single-label blocks");
        assert!(matches!(
            err,
            UntrustedOutboundError::LocalhostHostNotAllowed { .. }
        ));
    }

    #[test]
    fn allowlist_accepts_subdomains() {
        let policy = UntrustedOutboundPolicy {
            allowed_hosts: vec!["example.com".to_string()],
            ..Default::default()
        };
        let url = reqwest::Url::parse("https://api.example.com/mcp").expect("parse url");
        validate_untrusted_outbound_url(&policy, &url).expect("allowlisted host");
    }

    #[test]
    fn allowlist_requires_exact_ip_literal_match() {
        let policy = UntrustedOutboundPolicy {
            allowed_hosts: vec!["2.3.4".to_string(), "93.184.216.34".to_string()],
            ..Default::default()
        };
        let exact = reqwest::Url::parse("https://93.184.216.34/mcp").expect("parse url");
        validate_untrusted_outbound_url(&policy, &exact).expect("exact ip literal should pass");

        let suffix = reqwest::Url::parse("https://1.2.3.4/mcp").expect("parse url");
        let err =
            validate_untrusted_outbound_url(&policy, &suffix).expect_err("suffix match blocks");
        assert!(matches!(err, UntrustedOutboundError::HostNotAllowed { .. }));
    }

    #[test]
    fn literal_nat64_with_public_embedded_ipv4_is_allowed() {
        let policy = UntrustedOutboundPolicy::default();
        let url = reqwest::Url::parse("https://[64:ff9b::0808:0808]/mcp").expect("parse url");
        validate_untrusted_outbound_url(&policy, &url).expect("public embedded ipv4");
    }

    #[tokio::test]
    async fn dns_check_blocks_localhost_without_private_ip_override() {
        let policy = UntrustedOutboundPolicy {
            allow_localhost: true,
            dns_check: true,
            ..Default::default()
        };
        let url = reqwest::Url::parse("https://localhost/mcp").expect("parse url");
        let err = validate_untrusted_outbound_url_dns(&policy, &url)
            .await
            .expect_err("expected dns rejection");
        assert!(matches!(
            err,
            UntrustedOutboundError::ResolvedToNonGlobalIp { .. }
        ));
    }

    #[tokio::test]
    async fn dns_check_can_fail_open_on_timeout() {
        let policy = UntrustedOutboundPolicy {
            dns_check: true,
            dns_fail_open: true,
            dns_timeout: Duration::from_nanos(1),
            ..Default::default()
        };
        let url = reqwest::Url::parse("https://does-not-exist.invalid/mcp").expect("parse url");
        validate_untrusted_outbound_url_dns(&policy, &url)
            .await
            .expect("fail-open dns policy");
    }

    #[test]
    fn dns_results_with_private_ip_override_still_reject_loopback_for_non_localhost_hosts() {
        let policy = UntrustedOutboundPolicy {
            allow_private_ips: true,
            ..Default::default()
        };

        let err = validate_resolved_addrs(
            &policy,
            "example.test",
            [std::net::SocketAddr::from(([127, 0, 0, 1], 443))],
        )
        .expect_err("private-ip override must not allow loopback dns results");

        assert!(matches!(
            err,
            UntrustedOutboundError::ResolvedToNonGlobalIp { ip, .. }
                if ip == IpAddr::from([127, 0, 0, 1])
        ));
    }

    #[test]
    fn dns_results_reject_loopback_even_for_localhost_hosts() {
        let policy = UntrustedOutboundPolicy {
            allow_localhost: true,
            ..Default::default()
        };

        let err = validate_resolved_addrs(
            &policy,
            "localhost",
            [std::net::SocketAddr::from(([127, 0, 0, 1], 443))],
        )
        .expect_err("dns validation must still reject loopback answers");

        assert!(matches!(
            err,
            UntrustedOutboundError::ResolvedToNonGlobalIp { ip, .. }
                if ip == IpAddr::from([127, 0, 0, 1])
        ));
    }

    #[test]
    fn resolved_always_disallowed_ip_is_rejected_even_with_private_ip_override() {
        let policy = UntrustedOutboundPolicy {
            allow_private_ips: true,
            ..Default::default()
        };

        let err = validate_resolved_addrs(
            &policy,
            "example.test",
            [std::net::SocketAddr::from(([0, 0, 0, 0], 443))],
        )
        .expect_err("always-disallowed ip must still be rejected");

        assert!(matches!(
            err,
            UntrustedOutboundError::ResolvedToNonGlobalIp { ip, .. }
                if ip == IpAddr::from([0, 0, 0, 0])
        ));
    }

    #[test]
    fn private_ip_override_does_not_allow_loopback_ip_literals() {
        let policy = UntrustedOutboundPolicy {
            allow_private_ips: true,
            ..Default::default()
        };
        let url = reqwest::Url::parse("https://127.0.0.1/mcp").expect("parse url");
        let err = validate_untrusted_outbound_url(&policy, &url)
            .expect_err("loopback ip literal must still be rejected");
        assert!(matches!(
            err,
            UntrustedOutboundError::NonGlobalIpNotAllowed { .. }
        ));
    }

    #[test]
    fn private_ip_override_still_allows_private_ip_literals() {
        let policy = UntrustedOutboundPolicy {
            allow_private_ips: true,
            ..Default::default()
        };
        let url = reqwest::Url::parse("https://10.0.0.5/mcp").expect("parse url");
        validate_untrusted_outbound_url(&policy, &url)
            .expect("private-ip override should still allow RFC1918 literals");
    }
}
