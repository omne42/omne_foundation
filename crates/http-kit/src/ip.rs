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
    match normalize_ip(ip) {
        IpAddr::V4(ip) => is_non_global_ipv4(ip),
        IpAddr::V6(ip) => is_non_global_ipv6(ip),
    }
}

pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    !is_non_global_ip(ip)
}

fn is_non_global_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_documentation()
    {
        return true;
    }

    let [a, b, c, _d] = ip.octets();

    if a == 0 {
        return true;
    }

    if a == 100 && (64..=127).contains(&b) {
        return true;
    }

    if a == 192 && b == 0 && c == 0 {
        return true;
    }

    if (a, b, c) == (192, 88, 99) {
        return true;
    }

    if (a, b, c) == (192, 31, 196) {
        return true;
    }

    if (a, b, c) == (192, 52, 193) {
        return true;
    }

    if (a, b, c) == (192, 175, 48) {
        return true;
    }

    if a == 198 && (18..=19).contains(&b) {
        return true;
    }

    if a >= 240 {
        return true;
    }

    false
}

fn is_non_global_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
    {
        return true;
    }

    let bytes = ip.octets();

    if bytes[0] == 0xfe && (bytes[1] & 0xc0) == 0xc0 {
        return true;
    }

    if bytes[..8] == [0x01, 0x00, 0, 0, 0, 0, 0, 0] {
        return true;
    }

    if bytes[..6] == [0x20, 0x01, 0x00, 0x02, 0x00, 0x00] {
        return true;
    }

    if bytes[0] == 0x20 && bytes[1] == 0x01 && bytes[2] == 0x0d && bytes[3] == 0xb8 {
        return true;
    }

    false
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
