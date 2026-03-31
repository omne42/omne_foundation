use std::time::Duration;

use crate::Event;
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::webhook_common::JsonWebhookEndpoint;
use crate::sinks::{BoxFuture, Sink};
use http_kit::{ensure_http_success, redact_url, redact_url_str};

const DISCORD_ALLOWED_HOSTS: [&str; 2] = ["discord.com", "discordapp.com"];

#[non_exhaustive]
#[derive(Clone)]
pub struct DiscordWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for DiscordWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl DiscordWebhookConfig {
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

pub struct DiscordWebhookSink {
    endpoint: JsonWebhookEndpoint,
    max_chars: usize,
}

impl std::fmt::Debug for DiscordWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordWebhookSink")
            .field("webhook_url", &redact_url(self.endpoint.url()))
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}

impl DiscordWebhookSink {
    pub fn new(config: DiscordWebhookConfig) -> crate::Result<Self> {
        let endpoint = JsonWebhookEndpoint::new_validated_https(
            &config.webhook_url,
            &DISCORD_ALLOWED_HOSTS,
            "/api/webhooks/",
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
        serde_json::json!({ "content": text })
    }
}

impl Sink for DiscordWebhookSink {
    fn name(&self) -> &'static str {
        "discord"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let payload = Self::build_payload(event, self.max_chars);
            let resp = self.endpoint.post_json(&payload, "discord webhook").await?;
            Ok(ensure_http_success(resp, "discord webhook").await?)
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

        let payload = DiscordWebhookSink::build_payload(&event, 2000);
        let text = payload["content"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = DiscordWebhookConfig::new("http://discord.com/api/webhooks/x/y");
        let err = DiscordWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = DiscordWebhookConfig::new("https://example.com/api/webhooks/x/y");
        let err = DiscordWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_path() {
        let cfg = DiscordWebhookConfig::new("https://discord.com/api/x/y");
        let err = DiscordWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let url = "https://discord.com/api/webhooks/secret/token";
        let cfg = DiscordWebhookConfig::new(url);
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("discord.com"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = DiscordWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("discord.com"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }
}
