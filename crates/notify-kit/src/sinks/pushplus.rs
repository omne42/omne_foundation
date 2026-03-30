use std::time::Duration;

use crate::Event;
use crate::SecretString;
use crate::sinks::text::{TextLimits, format_event_body_and_tags_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};
use http_kit::{
    HttpClientOptions, HttpClientProfile, build_http_client_profile, parse_and_validate_https_url,
    read_json_body_after_http_success, redact_url, send_reqwest, validate_url_path_prefix,
};

const PUSHPLUS_ALLOWED_HOSTS: [&str; 1] = ["www.pushplus.plus"];

#[non_exhaustive]
#[derive(Clone)]
pub struct PushPlusConfig {
    pub token: SecretString,
    pub channel: Option<String>,
    pub template: Option<String>,
    pub topic: Option<String>,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for PushPlusConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushPlusConfig")
            .field("token", &"<redacted>")
            .field("channel", &self.channel)
            .field("template", &self.template)
            .field("topic", &self.topic)
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl PushPlusConfig {
    pub fn new(token: impl Into<SecretString>) -> Self {
        Self {
            token: token.into(),
            channel: None,
            template: Some("txt".to_string()),
            topic: None,
            timeout: Duration::from_secs(2),
            max_chars: 16 * 1024,
            enforce_public_ip: true,
        }
    }

    #[must_use]
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    #[must_use]
    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        self.template = Some(template.into());
        self
    }

    #[must_use]
    pub fn without_template(mut self) -> Self {
        self.template = None;
        self
    }

    #[must_use]
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
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

pub struct PushPlusSink {
    api_url: reqwest::Url,
    token: SecretString,
    channel: Option<String>,
    template: Option<String>,
    topic: Option<String>,
    http: HttpClientProfile,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for PushPlusSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushPlusSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("token", &"<redacted>")
            .field("channel", &self.channel)
            .field("template", &self.template)
            .field("topic", &self.topic)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl PushPlusSink {
    pub fn new(config: PushPlusConfig) -> crate::Result<Self> {
        let token = normalize_secret(config.token, "token")?;
        let channel = normalize_optional_trimmed(config.channel);
        let template = normalize_optional_trimmed(config.template);
        let topic = normalize_optional_trimmed(config.topic);

        let api_url = parse_and_validate_https_url(
            "https://www.pushplus.plus/send",
            &PUSHPLUS_ALLOWED_HOSTS,
        )?;
        validate_url_path_prefix(&api_url, "/send")?;

        let http = build_http_client_profile(&HttpClientOptions {
            timeout: Some(config.timeout),
            ..Default::default()
        })?;
        Ok(Self {
            api_url,
            token,
            channel,
            template,
            topic,
            http,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(
        event: &Event,
        token: &str,
        channel: Option<&str>,
        template: Option<&str>,
        topic: Option<&str>,
        max_chars: usize,
    ) -> serde_json::Value {
        let title = truncate_chars(event.title().as_ref(), 256);
        let content = format_event_body_and_tags_limited(event, TextLimits::new(max_chars));

        let mut obj = serde_json::Map::with_capacity(6);
        obj.insert("token".to_string(), serde_json::json!(token));
        obj.insert("title".to_string(), serde_json::json!(title));
        obj.insert("content".to_string(), serde_json::json!(content));

        if let Some(channel) = channel {
            obj.insert("channel".to_string(), serde_json::json!(channel));
        }
        if let Some(template) = template {
            obj.insert("template".to_string(), serde_json::json!(template));
        }
        if let Some(topic) = topic {
            obj.insert("topic".to_string(), serde_json::json!(topic));
        }

        serde_json::Value::Object(obj)
    }
}

fn normalize_optional_trimmed(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn pushplus_api_error(code: i64, msg: &str) -> crate::Error {
    let msg = truncate_chars(msg, 200);
    if msg.is_empty() {
        return anyhow::anyhow!("pushplus api error: code={code} (response body omitted)").into();
    }
    anyhow::anyhow!("pushplus api error: code={code}, msg={msg}").into()
}

impl Sink for PushPlusSink {
    fn name(&self) -> &'static str {
        "pushplus"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = self
                .http
                .select_for_url(&self.api_url, self.enforce_public_ip)
                .await?;

            let payload = Self::build_payload(
                event,
                self.token.expose_secret(),
                self.channel.as_deref(),
                self.template.as_deref(),
                self.topic.as_deref(),
                self.max_chars,
            );

            let resp = send_reqwest(
                client.post(self.api_url.as_str()).json(&payload),
                "pushplus",
            )
            .await?;
            let body = read_json_body_after_http_success(resp, "pushplus").await?;

            let code = body["code"].as_i64().unwrap_or(-1);
            if code == 200 {
                return Ok(());
            }

            let msg = body["msg"].as_str().unwrap_or("");
            Err(pushplus_api_error(code, msg))
        })
    }
}

fn normalize_secret(secret: SecretString, field: &str) -> crate::Result<SecretString> {
    let secret = secret.expose_secret().trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("pushplus {field} must not be empty").into());
    }
    Ok(SecretString::new(secret))
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

        let payload =
            PushPlusSink::build_payload(&event, "tok", None, Some("txt"), None, 16 * 1024);
        assert_eq!(payload["token"].as_str().unwrap_or(""), "tok");
        assert_eq!(payload["title"].as_str().unwrap_or(""), "done");
        let content = payload["content"].as_str().unwrap_or("");
        assert!(content.contains("ok"));
        assert!(content.contains("thread_id=t1"));
        assert_eq!(payload["template"].as_str().unwrap_or(""), "txt");
    }

    #[test]
    fn debug_redacts_token() {
        let cfg = PushPlusConfig::new("tok_secret");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("tok_secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = PushPlusSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("tok_secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("pushplus.plus"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn rejects_empty_token() {
        let cfg = PushPlusConfig::new("   ");
        let err = PushPlusSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("token"), "{err:#}");
    }

    #[test]
    fn trims_token_and_optional_fields() {
        let cfg = PushPlusConfig::new(" tok ")
            .with_channel(" chan ")
            .with_template(" txt ")
            .with_topic(" topic ");
        let sink = PushPlusSink::new(cfg).expect("build sink");
        assert_eq!(sink.token.expose_secret(), "tok");
        assert_eq!(sink.channel.as_deref(), Some("chan"));
        assert_eq!(sink.template.as_deref(), Some("txt"));
        assert_eq!(sink.topic.as_deref(), Some("topic"));
    }

    #[test]
    fn pushplus_api_error_message_is_not_contradictory() {
        let err = pushplus_api_error(500, "failed");
        let msg = err.to_string();
        assert!(msg.contains("msg=failed"), "{msg}");
        assert!(!msg.contains("response body omitted"), "{msg}");
    }

    #[test]
    fn pushplus_api_error_message_uses_omitted_when_empty() {
        let err = pushplus_api_error(500, "");
        let msg = err.to_string();
        assert!(msg.contains("response body omitted"), "{msg}");
    }
}
