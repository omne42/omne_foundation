use std::time::Duration;

use crate::Event;
use crate::SecretString;
use crate::sinks::text::{TextLimits, format_event_body_and_tags_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};
use http_kit::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, HttpClientOptions, HttpClientProfile,
    build_http_client_profile, http_status_text_error, parse_and_validate_https_url,
    read_text_body_limited, redact_url, response_body_read_error, send_reqwest,
    validate_url_path_prefix,
};

const BARK_ALLOWED_HOSTS: [&str; 1] = ["api.day.app"];

#[non_exhaustive]
#[derive(Clone)]
pub struct BarkConfig {
    pub device_key: SecretString,
    pub group: Option<String>,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for BarkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarkConfig")
            .field("device_key", &"<redacted>")
            .field("group", &self.group)
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl BarkConfig {
    pub fn new(device_key: impl Into<SecretString>) -> Self {
        Self {
            device_key: device_key.into(),
            group: None,
            timeout: Duration::from_secs(2),
            max_chars: 8 * 1024,
            enforce_public_ip: true,
        }
    }

    #[must_use]
    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
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

pub struct BarkSink {
    api_url: reqwest::Url,
    device_key: SecretString,
    group: Option<String>,
    http: HttpClientProfile,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for BarkSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarkSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("device_key", &"<redacted>")
            .field("group", &self.group)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl BarkSink {
    pub fn new(config: BarkConfig) -> crate::Result<Self> {
        let device_key = normalize_secret(config.device_key, "device_key")?;
        let group = normalize_optional_trimmed(config.group);

        let api_url =
            parse_and_validate_https_url("https://api.day.app/push", &BARK_ALLOWED_HOSTS)?;
        validate_url_path_prefix(&api_url, "/push")?;

        let http = build_http_client_profile(&HttpClientOptions {
            timeout: Some(config.timeout),
            ..Default::default()
        })?;
        Ok(Self {
            api_url,
            device_key,
            group,
            http,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(
        event: &Event,
        device_key: &str,
        group: Option<&str>,
        max_chars: usize,
    ) -> serde_json::Value {
        let title = truncate_chars(event.title().as_ref(), 256);
        let body = format_event_body_and_tags_limited(event, TextLimits::new(max_chars));

        let mut obj = serde_json::Map::with_capacity(4);
        obj.insert("device_key".to_string(), serde_json::json!(device_key));
        obj.insert("title".to_string(), serde_json::json!(title));
        obj.insert("body".to_string(), serde_json::json!(body));
        if let Some(group) = group {
            obj.insert("group".to_string(), serde_json::json!(group));
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

fn normalize_secret(secret: SecretString, field: &str) -> crate::Result<SecretString> {
    let secret = secret.expose_secret().trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("bark {field} must not be empty").into());
    }
    Ok(SecretString::new(secret))
}

fn bark_api_error(code: i64, message: &str) -> crate::Error {
    let message = truncate_chars(message, 200);
    if message.is_empty() {
        return anyhow::anyhow!("bark api error: code={code} (response body omitted)").into();
    }
    anyhow::anyhow!("bark api error: code={code}, message={message}").into()
}

impl Sink for BarkSink {
    fn name(&self) -> &'static str {
        "bark"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = self
                .http
                .select_for_url(&self.api_url, self.enforce_public_ip)
                .await?;

            let payload = Self::build_payload(
                event,
                self.device_key.expose_secret(),
                self.group.as_deref(),
                self.max_chars,
            );

            let resp =
                send_reqwest(client.post(self.api_url.as_str()).json(&payload), "bark").await?;

            let status = resp.status();
            if !status.is_success() {
                let body = read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES)
                    .await
                    .map_err(|err| response_body_read_error("bark http error", status, &err))?;
                return Err(http_status_text_error("bark", status, &body).into());
            }

            let content_type_is_json = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';').next().is_some_and(|media_type| {
                        media_type.trim().eq_ignore_ascii_case("application/json")
                    })
                });

            let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await {
                Ok(body) => body,
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "bark api error: {status} (failed to read response body: {err})"
                    )
                    .into());
                }
            };
            let body = body.trim();
            if body.is_empty() {
                return Ok(());
            }

            let maybe_json = content_type_is_json || body.starts_with('{') || body.starts_with('[');
            if !maybe_json {
                return Ok(());
            }

            let body: serde_json::Value = serde_json::from_str(body)
                .map_err(|err| anyhow::anyhow!("decode json failed: {err}"))?;

            let Some(code) = body.get("code").and_then(|v| v.as_i64()) else {
                return Ok(());
            };
            if code == 200 {
                return Ok(());
            }

            let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
            Err(bark_api_error(code, message))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;
    use structured_text_kit::structured_text;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = BarkSink::build_payload(&event, "k", Some("g"), 8 * 1024);
        assert_eq!(payload["device_key"].as_str().unwrap_or(""), "k");
        assert_eq!(payload["title"].as_str().unwrap_or(""), "done");
        let body = payload["body"].as_str().unwrap_or("");
        assert!(body.contains("ok"));
        assert!(body.contains("thread_id=t1"));
        assert_eq!(payload["group"].as_str().unwrap_or(""), "g");
    }

    #[test]
    fn builds_payload_from_structured_only_event() {
        let title = structured_text!("notify.title", "repo" => "omne");
        let body = structured_text!("notify.body", "step" => "review");
        let tag = structured_text!("notify.tag", "value" => "t1");
        let event = Event::new_structured("turn_completed", Severity::Success, title.clone())
            .with_body_text(body.clone())
            .with_tag_text("thread_id", tag.clone());

        let payload = BarkSink::build_payload(&event, "k", None, 8 * 1024);
        assert_eq!(payload["title"].as_str().unwrap_or(""), "");
        assert_eq!(payload["body"].as_str().unwrap_or(""), "");
    }

    #[test]
    fn debug_redacts_device_key() {
        let cfg = BarkConfig::new("secret_key");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret_key"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = BarkSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret_key"), "{sink_dbg}");
        assert!(sink_dbg.contains("api.day.app"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn rejects_empty_device_key() {
        let cfg = BarkConfig::new("   ");
        let err = BarkSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("device_key"), "{err:#}");
    }

    #[test]
    fn trims_device_key_and_group() {
        let cfg = BarkConfig::new(" key ").with_group(" team ");
        let sink = BarkSink::new(cfg).expect("build sink");
        assert_eq!(sink.device_key.expose_secret(), "key");
        assert_eq!(sink.group.as_deref(), Some("team"));
    }

    #[test]
    fn bark_api_error_message_is_not_contradictory() {
        let err = bark_api_error(500, "boom");
        let msg = err.to_string();
        assert!(msg.contains("message=boom"), "{msg}");
        assert!(!msg.contains("response body omitted"), "{msg}");
    }
}
