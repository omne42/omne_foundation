use std::time::Duration;

use crate::Event;
use crate::sinks::text::{TextLimits, format_event_text_limited, truncate_chars};
use crate::sinks::webhook_common::JsonWebhookEndpoint;
use crate::sinks::{BoxFuture, Sink};
use http_kit::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, http_status_text_error, read_text_body_limited, redact_url,
    redact_url_str, response_body_read_error,
};

const SLACK_ALLOWED_HOSTS: [&str; 1] = ["hooks.slack.com"];

#[non_exhaustive]
#[derive(Clone)]
pub struct SlackWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for SlackWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl SlackWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            timeout: Duration::from_secs(2),
            max_chars: 4000,
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

pub struct SlackWebhookSink {
    endpoint: JsonWebhookEndpoint,
    max_chars: usize,
}

impl std::fmt::Debug for SlackWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackWebhookSink")
            .field("webhook_url", &redact_url(self.endpoint.url()))
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}

impl SlackWebhookSink {
    pub fn new(config: SlackWebhookConfig) -> crate::Result<Self> {
        let endpoint = JsonWebhookEndpoint::new_validated_https(
            &config.webhook_url,
            &SLACK_ALLOWED_HOSTS,
            "/services/",
            config.timeout,
            config.enforce_public_ip,
        )?;
        Ok(Self {
            endpoint,
            max_chars: config.max_chars,
        })
    }

    fn build_payload(event: &Event, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        serde_json::json!({ "text": text })
    }
}

impl Sink for SlackWebhookSink {
    fn name(&self) -> &'static str {
        "slack"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let payload = Self::build_payload(event, self.max_chars);
            let resp = self.endpoint.post_json(&payload, "slack webhook").await?;
            let status = resp.status();
            let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await {
                Ok(body) => body,
                Err(err) => {
                    if status.is_success() {
                        return Err(response_body_read_error(
                            "slack webhook api error",
                            status,
                            &err,
                        )
                        .into());
                    }
                    return Err(
                        response_body_read_error("slack webhook http error", status, &err).into(),
                    );
                }
            };
            let body = body.trim();

            if !status.is_success() {
                return Err(http_status_text_error("slack webhook", status, body).into());
            }

            if body.is_empty() || body.eq_ignore_ascii_case("ok") {
                return Ok(());
            }

            let summary = truncate_chars(body, 200);
            Err(anyhow::anyhow!("slack webhook api error: response={summary}").into())
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

        let payload = SlackWebhookSink::build_payload(&event, 4000);
        let text = payload["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = SlackWebhookConfig::new("http://hooks.slack.com/services/x/y/z");
        let err = SlackWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = SlackWebhookConfig::new("https://example.com/services/x/y/z");
        let err = SlackWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_path() {
        let cfg = SlackWebhookConfig::new("https://hooks.slack.com/api/x/y/z");
        let err = SlackWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let url = "https://hooks.slack.com/services/secret/token";
        let cfg = SlackWebhookConfig::new(url);
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("hooks.slack.com"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = SlackWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("hooks.slack.com"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }
}
