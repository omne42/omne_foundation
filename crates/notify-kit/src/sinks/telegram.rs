use std::time::Duration;

use crate::Event;
use crate::NotifySecret;
use crate::sinks::text::{TextLimits, format_event_text_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};
use http_kit::{
    HttpClientOptions, HttpClientProfile, build_http_client_profile,
    read_json_body_after_http_success, redact_url, send_reqwest,
};
use secret_kit::SecretString;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

#[non_exhaustive]
#[derive(Clone)]
pub struct TelegramBotConfig {
    pub bot_token: NotifySecret,
    pub chat_id: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for TelegramBotConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBotConfig")
            .field("bot_token", &"<redacted>")
            .field("chat_id", &self.chat_id)
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl TelegramBotConfig {
    pub fn new(bot_token: impl Into<NotifySecret>, chat_id: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            chat_id: chat_id.into(),
            timeout: Duration::from_secs(2),
            max_chars: 4096,
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

pub struct TelegramBotSink {
    api_base: reqwest::Url,
    bot_token: SecretString,
    chat_id: String,
    http: HttpClientProfile,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for TelegramBotSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBotSink")
            .field("api_base", &redact_url(&self.api_base))
            .field("bot_token", &"<redacted>")
            .field("chat_id", &self.chat_id)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl TelegramBotSink {
    pub fn new(config: TelegramBotConfig) -> crate::Result<Self> {
        let bot_token = normalize_secret(config.bot_token, "bot_token")?;
        let chat_id = config.chat_id.trim();
        if chat_id.is_empty() {
            return Err(anyhow::anyhow!("telegram chat_id must not be empty").into());
        }

        let api_base = reqwest::Url::parse(TELEGRAM_API_BASE)
            .map_err(|err| anyhow::anyhow!("invalid telegram api base url: {err}"))?;
        let http = build_http_client_profile(&HttpClientOptions {
            timeout: Some(config.timeout),
            ..Default::default()
        })?;
        Ok(Self {
            api_base,
            bot_token,
            chat_id: chat_id.to_string(),
            http,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_api_url(
        api_base: &reqwest::Url,
        bot_token: &SecretString,
    ) -> crate::Result<reqwest::Url> {
        let mut api_url = api_base.clone();
        let bot_segment = format!("bot{}", bot_token.expose_secret());
        api_url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid telegram api base url"))?
            .push(&bot_segment)
            .push("sendMessage");
        Ok(api_url)
    }

    fn build_payload(event: &Event, chat_id: &str, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        let mut obj = serde_json::Map::with_capacity(3);
        obj.insert("chat_id".to_string(), serde_json::json!(chat_id));
        obj.insert("text".to_string(), serde_json::json!(text));
        obj.insert(
            "disable_web_page_preview".to_string(),
            serde_json::json!(true),
        );
        serde_json::Value::Object(obj)
    }

    fn build_api_error(body: &serde_json::Value) -> crate::Error {
        let code = body["error_code"].as_i64();
        let description = body["description"].as_str().unwrap_or("");
        let description = truncate_chars(description, 200);
        if let Some(code) = code {
            if !description.is_empty() {
                return anyhow::anyhow!("telegram api error: {code}, description={description}")
                    .into();
            }
            return anyhow::anyhow!("telegram api error: {code}").into();
        }

        if !description.is_empty() {
            return anyhow::anyhow!("telegram api error: description={description}").into();
        }

        anyhow::anyhow!("telegram api error").into()
    }
}

impl Sink for TelegramBotSink {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let payload = Self::build_payload(event, &self.chat_id, self.max_chars);
            let api_url = Self::build_api_url(&self.api_base, &self.bot_token)?;
            let client = self
                .http
                .select_for_url(&api_url, self.enforce_public_ip)
                .await?;

            let resp =
                send_reqwest(client.post(api_url.as_str()).json(&payload), "telegram").await?;
            let body = read_json_body_after_http_success(resp, "telegram").await?;

            let ok = body["ok"].as_bool().unwrap_or(false);
            if ok {
                return Ok(());
            }

            Err(Self::build_api_error(&body))
        })
    }
}

fn normalize_secret(secret: NotifySecret, field: &str) -> crate::Result<SecretString> {
    let secret = secret.expose_secret().trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("telegram {field} must not be empty").into());
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

        let payload = TelegramBotSink::build_payload(&event, "123", 4096);
        let text = payload["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
        assert_eq!(payload["chat_id"].as_str().unwrap_or(""), "123");
    }

    #[test]
    fn debug_redacts_bot_token() {
        let cfg = TelegramBotConfig::new("token:secret", "123");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("token:secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = TelegramBotSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("token:secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("api.telegram.org"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn bot_token_cannot_inject_url_structure() {
        let cfg = TelegramBotConfig::new("a/b?c#d", "123");
        let sink = TelegramBotSink::new(cfg).expect("build sink");
        let api_url = TelegramBotSink::build_api_url(&sink.api_base, &sink.bot_token)
            .expect("build request url");
        assert_eq!(api_url.scheme(), "https");
        assert_eq!(api_url.host_str().unwrap_or(""), "api.telegram.org");
        assert!(api_url.query().is_none(), "query must be none");
        assert!(api_url.fragment().is_none(), "fragment must be none");

        let path = api_url.path();
        assert!(path.starts_with("/bot"), "{path}");
        assert!(path.ends_with("/sendMessage"), "{path}");
    }

    #[test]
    fn trims_bot_token_and_chat_id() {
        let cfg = TelegramBotConfig::new(" token:secret ", " 123 ");
        let sink = TelegramBotSink::new(cfg).expect("build sink");
        assert_eq!(sink.chat_id, "123");
        assert_eq!(sink.bot_token.expose_secret(), "token:secret");
        let api_url = TelegramBotSink::build_api_url(&sink.api_base, &sink.bot_token)
            .expect("build request url");
        assert!(api_url.path().starts_with("/bot"), "{}", api_url.path());
        assert!(
            api_url.path().ends_with("/sendMessage"),
            "{}",
            api_url.path()
        );
        assert!(!api_url.as_str().contains("%20"), "{}", api_url.as_str());
    }

    #[test]
    fn telegram_api_error_message_with_description_is_not_contradictory() {
        let body = serde_json::json!({
            "ok": false,
            "error_code": 400,
            "description": "Bad Request: bad things happened",
        });
        let err = TelegramBotSink::build_api_error(&body);
        let msg = err.to_string();
        assert!(msg.contains("telegram api error: 400"), "{msg}");
        assert!(
            msg.contains("description=Bad Request: bad things happened"),
            "{msg}"
        );
        assert!(!msg.contains("response body omitted"), "{msg}");
    }

    #[test]
    fn telegram_api_error_message_uses_plain_code_without_omitted() {
        let body = serde_json::json!({
            "ok": false,
            "error_code": 401,
        });
        let err = TelegramBotSink::build_api_error(&body);
        let msg = err.to_string();
        assert_eq!(msg, "telegram api error: 401");
        assert!(!msg.contains("response body omitted"), "{msg}");
    }
}
