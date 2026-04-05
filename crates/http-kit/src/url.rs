use crate::error::{ErrorKind, tagged_message};

pub fn parse_and_validate_https_url_basic(url_str: &str) -> crate::Result<reqwest::Url> {
    let url = reqwest::Url::parse(url_str)
        .map_err(|err| tagged_message(ErrorKind::InvalidInput, format!("invalid url: {err}")))?;

    if url.scheme() != "https" {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url must use https",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url must not contain credentials",
        ));
    }

    let Some(host) = url.host_str() else {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url must have a host",
        ));
    };
    if host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok() {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url host is not allowed",
        ));
    }

    if let Some(port) = url.port() {
        if port != 443 {
            return Err(tagged_message(
                ErrorKind::InvalidInput,
                "url port is not allowed",
            ));
        }
    }

    Ok(url)
}

pub fn parse_and_validate_https_url(
    url_str: &str,
    allowed_hosts: &[&str],
) -> crate::Result<reqwest::Url> {
    let url = parse_and_validate_https_url_basic(url_str)?;
    let Some(host) = url.host_str() else {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url must have a host",
        ));
    };

    if !allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url host is not allowed",
        ));
    }

    Ok(url)
}

pub fn redact_url_str(url_str: &str) -> String {
    let Ok(url) = reqwest::Url::parse(url_str) else {
        return "<redacted>".to_string();
    };
    redact_url(&url)
}

pub fn redact_url(url: &reqwest::Url) -> String {
    match (url.scheme(), url.host_str()) {
        (scheme, Some(host)) => format!("{scheme}://{host}/<redacted>"),
        _ => "<redacted>".to_string(),
    }
}

pub fn redact_url_for_error(url: &reqwest::Url) -> String {
    let mut url = url.clone();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

pub fn redact_reqwest_error(err: &reqwest::Error) -> String {
    let mut msg = err.to_string();
    let Some(url) = err.url() else {
        return msg;
    };

    let full = url.as_str();
    let redacted = redact_url_for_error(url);
    msg = msg.replace(full, &redacted);
    msg
}

pub fn validate_url_path_prefix(url: &reqwest::Url, prefix: &str) -> crate::Result<()> {
    let path = url.path();
    if prefix.is_empty() {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url path is not allowed",
        ));
    }

    if prefix.ends_with('/') {
        if path.starts_with(prefix) {
            return Ok(());
        }
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url path is not allowed",
        ));
    }

    if path == prefix {
        return Ok(());
    }

    let Some(next) = path.as_bytes().get(prefix.len()) else {
        return Err(tagged_message(
            ErrorKind::InvalidInput,
            "url path is not allowed",
        ));
    };

    if path.starts_with(prefix) && *next == b'/' {
        return Ok(());
    }

    Err(tagged_message(
        ErrorKind::InvalidInput,
        "url path is not allowed",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_str_never_leaks_path_or_query() {
        let url = "https://hooks.slack.com/services/secret?token=top";
        let redacted = redact_url_str(url);
        assert!(!redacted.contains("secret"), "{redacted}");
        assert!(!redacted.contains("token"), "{redacted}");
        assert!(redacted.contains("hooks.slack.com"), "{redacted}");
        assert!(redacted.contains("<redacted>"), "{redacted}");
    }

    #[test]
    fn redact_url_for_error_removes_credentials_path_and_query() {
        let url = reqwest::Url::parse("https://user:pass@example.com/services/secret?token=top")
            .expect("parse url");
        let redacted = redact_url_for_error(&url);
        assert_eq!(redacted, "https://example.com/");
    }

    #[test]
    fn rejects_credentials() {
        let err = parse_and_validate_https_url(
            "https://u:p@hooks.slack.com/services/x",
            &["hooks.slack.com"],
        )
        .expect_err("expected invalid url");
        assert!(err.to_string().contains("credentials"), "{err:#}");
    }

    #[test]
    fn redact_url_for_error_preserves_origin_without_path_or_query() {
        let url =
            reqwest::Url::parse("https://user:pass@example.com:444/path?q=1#frag").expect("url");
        let redacted = redact_url_for_error(&url);
        assert_eq!(redacted, "https://example.com:444/");
    }

    #[test]
    fn rejects_non_443_port() {
        let err = parse_and_validate_https_url(
            "https://hooks.slack.com:444/services/x",
            &["hooks.slack.com"],
        )
        .expect_err("expected invalid url");
        assert!(err.to_string().contains("port"), "{err:#}");
    }

    #[test]
    fn path_prefix_is_segment_boundary_matched() {
        let url = reqwest::Url::parse("https://example.com/send").expect("parse url");
        validate_url_path_prefix(&url, "/send").expect("exact match");

        let url = reqwest::Url::parse("https://example.com/send/ok").expect("parse url");
        validate_url_path_prefix(&url, "/send").expect("segment match");

        let url = reqwest::Url::parse("https://example.com/sendMessage").expect("parse url");
        validate_url_path_prefix(&url, "/send").expect_err("should not match prefix substring");

        let url = reqwest::Url::parse("https://example.com/services/x").expect("parse url");
        validate_url_path_prefix(&url, "/services/").expect("trailing slash prefix");

        let url = reqwest::Url::parse("https://example.com/servicesX").expect("parse url");
        validate_url_path_prefix(&url, "/services/").expect_err("trailing slash prevents match");
    }
}
