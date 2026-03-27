use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(ip) => IpAddr::V4(ip),
        IpAddr::V6(ip) => embedded_ipv4_from_ipv6(ip).map_or(IpAddr::V6(ip), IpAddr::V4),
    }
}

pub(crate) fn is_always_disallowed_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(ip) => ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified(),
        IpAddr::V6(ip) => ip.is_multicast() || ip.is_unspecified(),
    }
}

pub(crate) fn is_non_global_ip(ip: IpAddr) -> bool {
    is_non_global_normalized_ip(normalize_ip(ip))
}

pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    let ip = normalize_ip(ip);
    !is_always_disallowed_normalized_ip(ip) && !is_non_global_normalized_ip(ip)
}

fn is_always_disallowed_normalized_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified(),
        IpAddr::V6(ip) => ip.is_multicast() || ip.is_unspecified(),
    }
}

fn is_non_global_normalized_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_non_global_ipv4(ip),
        IpAddr::V6(ip) => is_non_global_ipv6(ip),
    }
}

fn is_non_global_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();

    a == 0
        || ip.is_private()
        || is_shared_ipv4(ip)
        || ip.is_loopback()
        || ip.is_link_local()
        || ((a, b, c) == (192, 0, 0) && ip.octets()[3] != 9 && ip.octets()[3] != 10)
        || (a, b, c) == (192, 88, 99)
        || ip.is_documentation()
        || is_benchmarking_ipv4(ip)
        || is_reserved_ipv4(ip)
        || ip.is_broadcast()
}

fn is_non_global_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || is_ipv4_mapped_ipv6(ip)
        || is_ipv4_ipv6_translation_prefix(ip)
        || is_discard_only_ipv6(ip)
        || is_non_global_ietf_protocol_assignment_ipv6(ip)
        || is_6to4_ipv6(ip)
        || is_documentation_ipv6(ip)
        || is_srv6_sid_ipv6(ip)
        || is_site_local_ipv6(ip)
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_unspecified()
}

fn embedded_ipv4_from_ipv6(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = addr.to_ipv4() {
        return Some(v4);
    }

    let bytes = addr.octets();

    if bytes[..12] == [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0] {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }

    if bytes[0] == 0x20 && bytes[1] == 0x02 {
        return Some(Ipv4Addr::new(bytes[2], bytes[3], bytes[4], bytes[5]));
    }

    None
}

fn is_shared_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (b & 0b1100_0000) == 0b0100_0000
}

fn is_benchmarking_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 198 && (b & 0xfe) == 18
}

fn is_reserved_ipv4(ip: Ipv4Addr) -> bool {
    (ip.octets()[0] & 0xf0) == 0xf0 && !ip.is_broadcast()
}

fn is_ipv4_mapped_ipv6(ip: Ipv6Addr) -> bool {
    matches!(ip.segments(), [0, 0, 0, 0, 0, 0xffff, _, _])
}

fn is_ipv4_ipv6_translation_prefix(ip: Ipv6Addr) -> bool {
    matches!(ip.segments(), [0x64, 0xff9b, 1, _, _, _, _, _])
}

fn is_discard_only_ipv6(ip: Ipv6Addr) -> bool {
    matches!(ip.segments(), [0x100, 0, 0, 0, _, _, _, _])
}

fn is_non_global_ietf_protocol_assignment_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    if !(segments[0] == 0x2001 && segments[1] < 0x0200) {
        return false;
    }

    let raw = u128::from_be_bytes(ip.octets());
    if raw == 0x2001_0001_0000_0000_0000_0000_0000_0001
        || raw == 0x2001_0001_0000_0000_0000_0000_0000_0002
        || raw == 0x2001_0001_0000_0000_0000_0000_0000_0003
    {
        return false;
    }

    if matches!(segments, [0x2001, 3, _, _, _, _, _, _]) {
        return false;
    }

    if matches!(segments, [0x2001, 4, 0x112, _, _, _, _, _]) {
        return false;
    }

    if matches!(segments, [0x2001, value, _, _, _, _, _, _] if (0x20..=0x2f).contains(&value)) {
        return false;
    }

    if matches!(segments, [0x2001, value, _, _, _, _, _, _] if (0x30..=0x3f).contains(&value)) {
        return false;
    }

    true
}

fn is_6to4_ipv6(ip: Ipv6Addr) -> bool {
    matches!(ip.segments(), [0x2002, _, _, _, _, _, _, _])
}

fn is_documentation_ipv6(ip: Ipv6Addr) -> bool {
    matches!(
        ip.segments(),
        [0x2001, 0x0db8, ..] | [0x3fff, 0..=0x0fff, ..]
    )
}

fn is_srv6_sid_ipv6(ip: Ipv6Addr) -> bool {
    matches!(ip.segments(), [0x5f00, ..])
}

fn is_site_local_ipv6(ip: Ipv6Addr) -> bool {
    let bytes = ip.octets();
    bytes[0] == 0xfe && (bytes[1] & 0xc0) == 0xc0
}

#[cfg(test)]
mod tests {
    use super::{is_non_global_ip, is_public_ip};
    use std::net::IpAddr;
    use std::str::FromStr;

    #[test]
    fn ipv4_special_use_exceptions_remain_public() {
        assert!(is_public_ip(IpAddr::from_str("192.0.0.9").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("192.0.0.10").unwrap()));
        assert!(!is_non_global_ip(IpAddr::from_str("192.0.0.9").unwrap()));
        assert!(!is_non_global_ip(IpAddr::from_str("192.0.0.10").unwrap()));
    }

    #[test]
    fn ipv4_anycast_ranges_are_not_misclassified_as_private() {
        assert!(is_public_ip(IpAddr::from_str("192.31.196.1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("192.52.193.1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("192.175.48.1").unwrap()));
    }

    #[test]
    fn ipv6_ietf_protocol_assignments_default_to_non_global() {
        assert!(!is_public_ip(IpAddr::from_str("2001::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2001:40::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("5f00::1").unwrap()));
    }

    #[test]
    fn ipv6_ietf_protocol_assignment_exceptions_remain_public() {
        assert!(is_public_ip(IpAddr::from_str("2001:1::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:1::2").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:1::3").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:3::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:4:112::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:20::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:30::1").unwrap()));
    }

    #[test]
    fn ipv6_documentation_and_translation_ranges_are_non_global() {
        assert!(!is_public_ip(IpAddr::from_str("2001:db8::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("3fff::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("100::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2001:40::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("fec0::1").unwrap()));
    }
}
