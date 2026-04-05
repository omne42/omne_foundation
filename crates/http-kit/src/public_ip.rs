use std::collections::HashSet;
use std::net::SocketAddr;

use crate::error::{ErrorKind, tagged_message};
use crate::ip::is_public_ip;

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
            return Err(tagged_message(
                ErrorKind::InvalidInput,
                "resolved ip is not allowed",
            ));
        }
        if uniq.insert(addr) {
            out.push(addr);
        }
    }

    if !seen_any {
        return Err(tagged_message(ErrorKind::Transport, "dns lookup failed"));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
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
        assert!(is_public_ip(IpAddr::from_str("192.0.0.9").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("192.0.0.10").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::c000:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:c000:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.88.99.1").unwrap()));
        assert!(!is_public_ip(
            IpAddr::from_str("64:ff9b::c058:6301").unwrap()
        ));
        assert!(!is_public_ip(
            IpAddr::from_str("2002:c058:6301::1").unwrap()
        ));
        assert!(is_public_ip(IpAddr::from_str("192.31.196.1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("192.52.193.1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("192.175.48.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("fec0::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("100::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2001:2::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2001:40::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:1::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:1::2").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:1::3").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:3::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:4:112::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:20::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2001:30::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("3fff::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("5f00::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("169.254.1.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("8.8.8.8").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("::ffff:8.8.8.8").unwrap()));
        assert!(is_public_ip(
            IpAddr::from_str("2001:4860:4860::8888").unwrap()
        ));
        assert!(is_public_ip(IpAddr::from_str("::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("64:ff9b::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2002:808:808::1").unwrap()));
    }
}
