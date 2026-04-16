use crate::error::{ErrorKind, tagged_message};
use crate::outbound_policy::{host_for_ip_literal, is_local_or_single_label_host};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebsocketBaseUrlRewrite {
    HttpToWebsocket,
    HttpsToSecureWebsocket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebsocketBaseUrlResolution {
    pub base_url: String,
    pub rewrite: Option<WebsocketBaseUrlRewrite>,
}

pub fn resolve_websocket_base_url(base_url: &str) -> WebsocketBaseUrlResolution {
    let base_url = base_url.trim();
    if let Some(rest) = base_url.strip_prefix("https://") {
        return WebsocketBaseUrlResolution {
            base_url: format!("wss://{rest}"),
            rewrite: Some(WebsocketBaseUrlRewrite::HttpsToSecureWebsocket),
        };
    }
    if let Some(rest) = base_url.strip_prefix("http://") {
        return WebsocketBaseUrlResolution {
            base_url: format!("ws://{rest}"),
            rewrite: Some(WebsocketBaseUrlRewrite::HttpToWebsocket),
        };
    }
    WebsocketBaseUrlResolution {
        base_url: base_url.to_string(),
        rewrite: None,
    }
}

pub fn join_api_base_url_path(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path_no_leading_slash = path.strip_prefix('/').unwrap_or(path);

    if base.ends_with("/v1") {
        if path_no_leading_slash == "v1" {
            return base.to_string();
        }
        if let Some(rest) = path_no_leading_slash.strip_prefix("v1/") {
            let mut out = String::with_capacity(base.len() + 1 + rest.len());
            out.push_str(base);
            out.push('/');
            out.push_str(rest);
            return out;
        }
    }

    if path.starts_with('/') {
        let mut out = String::with_capacity(base.len() + path.len());
        out.push_str(base);
        out.push_str(path);
        out
    } else {
        let mut out = String::with_capacity(base.len() + 1 + path.len());
        out.push_str(base);
        out.push('/');
        out.push_str(path);
        out
    }
}

pub fn append_url_query_params(base_url: String, query_params: &[(String, String)]) -> String {
    if query_params.is_empty() {
        return base_url;
    }

    let mut out = base_url;
    out.push(if out.contains('?') { '&' } else { '?' });
    for (idx, (name, value)) in query_params.iter().enumerate() {
        if idx > 0 {
            out.push('&');
        }
        out.push_str(name);
        out.push('=');
        out.push_str(value);
    }
    out
}

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
    let host_for_ip = host_for_ip_literal(host);
    if host_for_ip.parse::<std::net::IpAddr>().is_ok()
        || is_local_or_single_label_host(host, host_for_ip)
    {
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
    fn resolve_websocket_base_url_reports_rewrite_kind() {
        let secure = resolve_websocket_base_url("https://api.openai.com/v1");
        assert_eq!(secure.base_url, "wss://api.openai.com/v1");
        assert_eq!(
            secure.rewrite,
            Some(WebsocketBaseUrlRewrite::HttpsToSecureWebsocket)
        );

        let insecure = resolve_websocket_base_url("http://localhost:8080/v1");
        assert_eq!(insecure.base_url, "ws://localhost:8080/v1");
        assert_eq!(
            insecure.rewrite,
            Some(WebsocketBaseUrlRewrite::HttpToWebsocket)
        );

        let passthrough = resolve_websocket_base_url("wss://proxy.example/v1");
        assert_eq!(passthrough.base_url, "wss://proxy.example/v1");
        assert_eq!(passthrough.rewrite, None);
    }

    #[test]
    fn join_api_base_url_path_respects_v1_join_ergonomics() {
        assert_eq!(
            join_api_base_url_path("http://localhost:8080/v1", "/v1/chat/completions"),
            "http://localhost:8080/v1/chat/completions"
        );
        assert_eq!(
            join_api_base_url_path("http://localhost:8080/v1", "v1/chat/completions"),
            "http://localhost:8080/v1/chat/completions"
        );
        assert_eq!(
            join_api_base_url_path("http://localhost:8080/v1", "/v1"),
            "http://localhost:8080/v1"
        );
    }

    #[test]
    fn append_url_query_params_appends_with_existing_separator() {
        assert_eq!(
            append_url_query_params(
                "https://example.com/path".to_string(),
                &[
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "2".to_string())
                ]
            ),
            "https://example.com/path?a=1&b=2"
        );
        assert_eq!(
            append_url_query_params(
                "https://example.com/path?x=0".to_string(),
                &[("a".to_string(), "1".to_string())]
            ),
            "https://example.com/path?x=0&a=1"
        );
    }

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
    fn rejects_local_and_single_label_hosts_in_basic_https_validation() {
        for url in [
            "https://localhost/hook",
            "https://demo.localhost/hook",
            "https://service.local/hook",
            "https://service.localdomain/hook",
            "https://internal/hook",
            "https://127.0.0.1/hook",
            "https://[::1]/hook",
        ] {
            let err = parse_and_validate_https_url_basic(url).expect_err("host should be rejected");
            assert!(
                err.to_string().contains("host is not allowed"),
                "url={url} err={err:#}"
            );
        }
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
