use std::time::Duration;

use crate::Event;
use crate::sinks::http::{
    build_http_client, parse_and_validate_https_url, parse_and_validate_https_url_basic,
    read_json_body_after_http_success, redact_url, select_http_client, send_reqwest,
    validate_url_path_prefix,
};
use crate::sinks::text::{TextLimits, format_event_body_and_tags_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};

const SERVERCHAN_TURBO_ALLOWED_HOSTS: [&str; 1] = ["sctapi.ftqq.com"];

#[non_exhaustive]
#[derive(Clone)]
pub struct ServerChanConfig {
    pub send_key: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for ServerChanConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerChanConfig")
            .field("send_key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl ServerChanConfig {
    pub fn new(send_key: impl Into<String>) -> Self {
        Self {
            send_key: send_key.into(),
            timeout: Duration::from_secs(2),
            max_chars: 16 * 1024,
            enforce_public_ip: true,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    #[must_use]
    pub fn with_public_ip_check(mut self, enforce_public_ip: bool) -> Self {
        self.enforce_public_ip = enforce_public_ip;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerChanKind {
    Turbo,
    Sc3,
}

pub struct ServerChanSink {
    api_url: reqwest::Url,
    kind: ServerChanKind,
    client: reqwest::Client,
    timeout: Duration,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for ServerChanSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerChanSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("kind", &self.kind)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl ServerChanSink {
    pub fn new(config: ServerChanConfig) -> crate::Result<Self> {
        let (kind, raw_api_url) = build_serverchan_url(&config.send_key)?;

        let api_url = match kind {
            ServerChanKind::Turbo => {
                let url = parse_and_validate_https_url(
                    raw_api_url.as_str(),
                    &SERVERCHAN_TURBO_ALLOWED_HOSTS,
                )?;
                validate_url_path_prefix(&url, "/")?;
                url
            }
            ServerChanKind::Sc3 => {
                let url = parse_and_validate_https_url_basic(raw_api_url.as_str())?;
                validate_url_path_prefix(&url, "/send/")?;
                url
            }
        };

        let client = build_http_client(config.timeout)?;
        Ok(Self {
            api_url,
            kind,
            client,
            timeout: config.timeout,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(event: &Event, max_chars: usize) -> serde_json::Value {
        let title = truncate_chars(&event.title, 256);
        let desp = format_event_body_and_tags_limited(event, TextLimits::new(max_chars));
        serde_json::json!({ "title": title, "desp": desp })
    }

    fn ensure_success_response(body: &serde_json::Value) -> crate::Result<()> {
        let Some(code) = body["code"].as_i64().or_else(|| body["errno"].as_i64()) else {
            return Err(anyhow::anyhow!(
                "serverchan api error: missing code (response body omitted)"
            )
            .into());
        };
        if code == 0 {
            return Ok(());
        }
        Err(anyhow::anyhow!("serverchan api error: code={code} (response body omitted)").into())
    }
}

fn normalize_serverchan_send_key(send_key: &str) -> crate::Result<&str> {
    let send_key = send_key.trim();
    if send_key.is_empty() {
        return Err(anyhow::anyhow!("serverchan send_key must not be empty").into());
    }
    if !send_key.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err(anyhow::anyhow!("invalid serverchan send_key").into());
    }
    Ok(send_key)
}

fn build_serverchan_url(send_key: &str) -> crate::Result<(ServerChanKind, reqwest::Url)> {
    let send_key = normalize_serverchan_send_key(send_key)?;

    if let Some(rest) = send_key.strip_prefix("sctp") {
        let Some(pos) = rest.find('t') else {
            return Err(anyhow::anyhow!("invalid serverchan send_key").into());
        };
        let (uid_str, tail) = rest.split_at(pos);
        if uid_str.is_empty() || !uid_str.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(anyhow::anyhow!("invalid serverchan send_key").into());
        }
        if tail.len() <= 1 {
            return Err(anyhow::anyhow!("invalid serverchan send_key").into());
        }
        let uid: u64 = uid_str
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid serverchan send_key"))?;

        let host = format!("{uid}.push.ft07.com");
        let mut url = reqwest::Url::parse(&format!("https://{host}/"))
            .map_err(|err| anyhow::anyhow!("invalid url: {err}"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid serverchan url"))?;
            segments.push("send");
            segments.push(&format!("{send_key}.send"));
        }
        return Ok((ServerChanKind::Sc3, url));
    }

    let mut url = reqwest::Url::parse("https://sctapi.ftqq.com/")
        .map_err(|err| anyhow::anyhow!("invalid url: {err}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid serverchan url"))?;
        segments.push(&format!("{send_key}.send"));
    }
    Ok((ServerChanKind::Turbo, url))
}

impl Sink for ServerChanSink {
    fn name(&self) -> &'static str {
        "serverchan"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = select_http_client(
                &self.client,
                self.timeout,
                &self.api_url,
                self.enforce_public_ip,
            )
            .await?;

            let payload = Self::build_payload(event, self.max_chars);

            let resp = send_reqwest(
                client.post(self.api_url.as_str()).json(&payload),
                "serverchan",
            )
            .await?;
            let body = read_json_body_after_http_success(resp, "serverchan").await?;
            Self::ensure_success_response(&body)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;
    use crate::sinks::http::redact_url_str;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = ServerChanSink::build_payload(&event, 16 * 1024);
        assert_eq!(payload["title"].as_str().unwrap_or(""), "done");
        let desp = payload["desp"].as_str().unwrap_or("");
        assert!(desp.contains("ok"));
        assert!(desp.contains("thread_id=t1"));
    }

    #[test]
    fn build_url_supports_turbo_and_sc3() {
        let (kind, url) = build_serverchan_url("SCT123tABC").expect("turbo url");
        assert_eq!(kind, ServerChanKind::Turbo);
        assert_eq!(url.host_str().unwrap_or(""), "sctapi.ftqq.com");
        assert!(url.path().ends_with(".send"));

        let (kind, url) = build_serverchan_url("sctp123tABC").expect("sc3 url");
        assert_eq!(kind, ServerChanKind::Sc3);
        assert_eq!(url.host_str().unwrap_or(""), "123.push.ft07.com");
        assert!(url.path().starts_with("/send/"));
        assert!(url.path().ends_with(".send"));
    }

    #[test]
    fn debug_redacts_send_key() {
        let cfg = ServerChanConfig::new("SCTsecret");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("SCTsecret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");
    }

    #[test]
    fn rejects_empty_send_key() {
        let cfg = ServerChanConfig::new("   ");
        let err = ServerChanSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("send_key"), "{err:#}");
    }

    #[test]
    fn redact_url_str_never_leaks_send_key() {
        let cfg = ServerChanConfig::new("SCTsecret");
        let (kind, url) = build_serverchan_url(&cfg.send_key).expect("build url");
        assert!(matches!(kind, ServerChanKind::Turbo | ServerChanKind::Sc3));
        let redacted = redact_url_str(url.as_str());
        assert!(!redacted.contains("SCTsecret"), "{redacted}");
        assert!(redacted.contains("<redacted>"), "{redacted}");
    }

    #[test]
    fn rejects_send_key_with_reserved_url_chars() {
        let cfg = ServerChanConfig::new("SCTbad?x=1");
        let err = ServerChanSink::new(cfg).expect_err("expected invalid send_key");
        assert!(
            err.to_string().contains("invalid serverchan send_key"),
            "{err:#}"
        );
    }

    #[test]
    fn rejects_sc3_send_key_without_suffix_code() {
        let cfg = ServerChanConfig::new("sctp123t");
        let err = ServerChanSink::new(cfg).expect_err("expected invalid send_key");
        assert!(
            err.to_string().contains("invalid serverchan send_key"),
            "{err:#}"
        );
    }

    #[test]
    fn response_requires_explicit_success_code() {
        let body = serde_json::json!({});
        let err =
            ServerChanSink::ensure_success_response(&body).expect_err("expected missing code");
        assert!(err.to_string().contains("missing code"), "{err:#}");
    }

    #[test]
    fn response_accepts_zero_code() {
        let body = serde_json::json!({ "code": 0 });
        ServerChanSink::ensure_success_response(&body).expect("expected success");

        let body = serde_json::json!({ "errno": 0 });
        ServerChanSink::ensure_success_response(&body).expect("expected success");
    }
}
