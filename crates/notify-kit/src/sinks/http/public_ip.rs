use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub(crate) fn validate_public_addrs<I>(addrs: I) -> crate::Result<Vec<SocketAddr>>
where
    I: IntoIterator<Item = SocketAddr>,
{
    let addrs = addrs.into_iter();
    let (lower, upper) = addrs.size_hint();
    let cap = upper.unwrap_or(lower);
    let mut out: Vec<SocketAddr> = Vec::with_capacity(cap);
    let mut uniq: HashSet<SocketAddr> = HashSet::with_capacity(cap);
    let mut seen_any = false;
    for addr in addrs {
        seen_any = true;
        if !is_public_ip(addr.ip()) {
            return Err(anyhow::anyhow!("resolved ip is not allowed").into());
        }
        if uniq.insert(addr) {
            out.push(addr);
        }
    }

    if !seen_any {
        return Err(anyhow::anyhow!("dns lookup failed").into());
    }

    Ok(out)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => is_public_ipv4(addr),
        IpAddr::V6(addr) => is_public_ipv6(addr),
    }
}

fn is_public_ipv4(addr: Ipv4Addr) -> bool {
    let [a, b, c, _d] = addr.octets();

    // Unspecified / "this host"
    if a == 0 {
        return false;
    }

    // IETF protocol assignments (RFC6890)
    if (a, b, c) == (192, 0, 0) {
        return false;
    }

    // Private ranges (RFC1918)
    if a == 10 {
        return false;
    }
    if a == 172 && (16..=31).contains(&b) {
        return false;
    }
    if a == 192 && b == 168 {
        return false;
    }

    // Carrier-grade NAT (RFC6598)
    if a == 100 && (64..=127).contains(&b) {
        return false;
    }

    // Loopback
    if a == 127 {
        return false;
    }

    // Link-local
    if a == 169 && b == 254 {
        return false;
    }

    // 6to4 relay anycast (RFC3068; deprecated)
    if (a, b, c) == (192, 88, 99) {
        return false;
    }

    // AS112 (RFC7534)
    if (a, b, c) == (192, 31, 196) {
        return false;
    }

    // AMT (RFC7450)
    if (a, b, c) == (192, 52, 193) {
        return false;
    }

    // Direct Delegation AS112 (RFC7535)
    if (a, b, c) == (192, 175, 48) {
        return false;
    }

    // Documentation ranges (RFC5737)
    if (a, b, c) == (192, 0, 2) || (a, b, c) == (198, 51, 100) || (a, b, c) == (203, 0, 113) {
        return false;
    }

    // Network interconnect device benchmark testing (RFC2544)
    if a == 198 && (b == 18 || b == 19) {
        return false;
    }

    // Multicast (224/4) and reserved (240/4)
    if a >= 224 {
        return false;
    }

    true
}

fn is_public_ipv6(addr: Ipv6Addr) -> bool {
    if let Some(v4) = ipv4_from_ipv6_mapped(addr) {
        return is_public_ipv4(v4);
    }

    if let Some(v4) = ipv4_from_nat64_well_known_prefix(addr) {
        return is_public_ipv4(v4);
    }

    if let Some(v4) = ipv4_from_6to4(addr) {
        return is_public_ipv4(v4);
    }

    let bytes = addr.octets();

    // IPv4-compatible IPv6 (::/96) is deprecated and should never be treated
    // as publicly routable for SSRF checks.
    if bytes[..12] == [0; 12] {
        return false;
    }

    // Unspecified :: / loopback ::1
    if addr.is_unspecified() || addr.is_loopback() {
        return false;
    }

    // Discard-only prefix 100::/64 (RFC6666)
    if bytes[..8] == [0x01, 0x00, 0, 0, 0, 0, 0, 0] {
        return false;
    }

    // Benchmarking 2001:2::/48 (RFC5180)
    if bytes[..6] == [0x20, 0x01, 0x00, 0x02, 0x00, 0x00] {
        return false;
    }

    // Multicast ff00::/8
    if bytes[0] == 0xff {
        return false;
    }

    // Unique local fc00::/7
    if (bytes[0] & 0xfe) == 0xfc {
        return false;
    }

    // Link-local fe80::/10
    if bytes[0] == 0xfe && (bytes[1] & 0xc0) == 0x80 {
        return false;
    }

    // Site-local fec0::/10 (deprecated; treat as non-public)
    if bytes[0] == 0xfe && (bytes[1] & 0xc0) == 0xc0 {
        return false;
    }

    // Documentation 2001:db8::/32
    if bytes[0] == 0x20 && bytes[1] == 0x01 && bytes[2] == 0x0d && bytes[3] == 0xb8 {
        return false;
    }

    true
}

fn ipv4_from_ipv6_mapped(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let bytes = addr.octets();
    // IPv4-mapped IPv6 (::ffff:0:0/96)
    if bytes[..10] == [0; 10] && bytes[10] == 0xff && bytes[11] == 0xff {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }
    None
}

fn ipv4_from_nat64_well_known_prefix(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let bytes = addr.octets();
    // NAT64 Well-Known Prefix (RFC6052): 64:ff9b::/96
    if bytes[..12] == [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0] {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }
    None
}

fn ipv4_from_6to4(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let bytes = addr.octets();
    // 6to4 (RFC3056; deprecated): 2002::/16 embeds an IPv4 address.
    if bytes[0] == 0x20 && bytes[1] == 0x02 {
        return Some(Ipv4Addr::new(bytes[2], bytes[3], bytes[4], bytes[5]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn ip_global_checks_work_for_common_ranges() {
        assert!(!is_public_ip(IpAddr::from_str("127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::7f00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::7f00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:7f00:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::a00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::a00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:a00:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::c000:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:c000:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.88.99.1").unwrap()));
        assert!(!is_public_ip(
            IpAddr::from_str("64:ff9b::c058:6301").unwrap()
        ));
        assert!(!is_public_ip(
            IpAddr::from_str("2002:c058:6301::1").unwrap()
        ));
        assert!(!is_public_ip(IpAddr::from_str("192.31.196.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.52.193.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.175.48.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("fec0::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("100::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2001:2::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("169.254.1.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("8.8.8.8").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("::ffff:8.8.8.8").unwrap()));
        assert!(is_public_ip(
            IpAddr::from_str("2001:4860:4860::8888").unwrap()
        ));
        assert!(!is_public_ip(IpAddr::from_str("::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("64:ff9b::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2002:808:808::1").unwrap()));
    }
}
