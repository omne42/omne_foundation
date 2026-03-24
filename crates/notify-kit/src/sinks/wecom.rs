use std::time::Duration;

use crate::Event;
use crate::sinks::http::{
    build_http_client, parse_and_validate_https_url, read_json_body_after_http_success, redact_url,
    redact_url_str, select_http_client, send_reqwest, validate_url_path_prefix,
};
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::{BoxFuture, Sink};

const WECOM_ALLOWED_HOSTS: [&str; 1] = ["qyapi.weixin.qq.com"];

#[non_exhaustive]
#[derive(Clone)]
pub struct WeComWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for WeComWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeComWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl WeComWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            timeout: Duration::from_secs(2),
            max_chars: 2000,
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

pub struct WeComWebhookSink {
    webhook_url: reqwest::Url,
    client: reqwest::Client,
    timeout: Duration,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for WeComWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeComWebhookSink")
            .field("webhook_url", &redact_url(&self.webhook_url))
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}

impl WeComWebhookSink {
    pub fn new(config: WeComWebhookConfig) -> crate::Result<Self> {
        let webhook_url = parse_and_validate_https_url(&config.webhook_url, &WECOM_ALLOWED_HOSTS)?;
        validate_url_path_prefix(&webhook_url, "/cgi-bin/webhook/send")?;
        let client = build_http_client(config.timeout)?;
        Ok(Self {
            webhook_url,
            client,
            timeout: config.timeout,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(event: &Event, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        serde_json::json!({
            "msgtype": "text",
            "text": { "content": text },
        })
    }
}

impl Sink for WeComWebhookSink {
    fn name(&self) -> &'static str {
        "wecom"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = select_http_client(
                &self.client,
                self.timeout,
                &self.webhook_url,
                self.enforce_public_ip,
            )
            .await?;
            let payload = Self::build_payload(event, self.max_chars);

            let resp = send_reqwest(
                client.post(self.webhook_url.as_str()).json(&payload),
                "wecom webhook",
            )
            .await?;

            let body = read_json_body_after_http_success(resp, "wecom webhook").await?;
            let errcode = body["errcode"].as_i64().unwrap_or(-1);
            if errcode == 0 {
                return Ok(());
            }

            Err(
                anyhow::anyhow!("wecom api error: errcode={errcode} (response body omitted)")
                    .into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = WeComWebhookSink::build_payload(&event, 2000);
        assert_eq!(payload["msgtype"].as_str().unwrap_or(""), "text");
        let text = payload["text"]["content"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = WeComWebhookConfig::new(
            "http://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret_key",
        );
        let err = WeComWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = WeComWebhookConfig::new("https://example.com/cgi-bin/webhook/send?key=x");
        let err = WeComWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_path() {
        let cfg = WeComWebhookConfig::new("https://qyapi.weixin.qq.com/cgi-bin/webhook/evil?key=x");
        let err = WeComWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let url = "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret_key";
        let cfg = WeComWebhookConfig::new(url);
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret_key"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("qyapi.weixin.qq.com"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = WeComWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret_key"), "{sink_dbg}");
        assert!(sink_dbg.contains("qyapi.weixin.qq.com"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }
}
