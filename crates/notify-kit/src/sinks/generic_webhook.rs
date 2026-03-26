use std::time::Duration;

use crate::Event;
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::{BoxFuture, Sink};
use http_kit::{
    build_http_client, ensure_http_success, parse_and_validate_https_url_basic, redact_url,
    redact_url_str, select_http_client, send_reqwest, validate_url_path_prefix,
};

#[non_exhaustive]
#[derive(Clone)]
pub struct GenericWebhookConfig {
    pub url: String,
    pub payload_field: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
    pub path_prefix: Option<String>,
    pub allowed_hosts: Vec<String>,
}

impl std::fmt::Debug for GenericWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericWebhookConfig")
            .field("url", &redact_url_str(&self.url))
            .field("payload_field", &self.payload_field)
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field("path_prefix", &self.path_prefix)
            .field("allowed_hosts", &self.allowed_hosts)
            .finish()
    }
}

impl GenericWebhookConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            payload_field: "text".to_string(),
            timeout: Duration::from_secs(2),
            max_chars: 16 * 1024,
            enforce_public_ip: true,
            path_prefix: None,
            allowed_hosts: Vec::new(),
        }
    }

    pub fn new_strict(
        url: impl Into<String>,
        path_prefix: impl Into<String>,
        allowed_hosts: Vec<String>,
    ) -> Self {
        Self {
            url: url.into(),
            payload_field: "text".to_string(),
            timeout: Duration::from_secs(2),
            max_chars: 16 * 1024,
            enforce_public_ip: true,
            path_prefix: Some(path_prefix.into()),
            allowed_hosts,
        }
    }

    #[must_use]
    pub fn with_payload_field(mut self, payload_field: impl Into<String>) -> Self {
        self.payload_field = payload_field.into();
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

    #[must_use]
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = Some(prefix.into());
        self
    }

    #[must_use]
    pub fn with_allowed_hosts(mut self, allowed_hosts: Vec<String>) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }
}

pub struct GenericWebhookSink {
    url: reqwest::Url,
    payload_field: String,
    client: reqwest::Client,
    timeout: Duration,
    max_chars: usize,
    enforce_public_ip: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenericWebhookValidationMode {
    Relaxed,
    Strict,
}

struct NormalizedGenericWebhookConfig {
    url: reqwest::Url,
    payload_field: String,
    timeout: Duration,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for GenericWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericWebhookSink")
            .field("url", &redact_url(&self.url))
            .field("payload_field", &self.payload_field)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl GenericWebhookSink {
    pub fn new(config: GenericWebhookConfig) -> crate::Result<Self> {
        Self::build_from_config(config, GenericWebhookValidationMode::Relaxed)
    }

    pub fn new_strict(config: GenericWebhookConfig) -> crate::Result<Self> {
        Self::build_from_config(config, GenericWebhookValidationMode::Strict)
    }

    fn build_from_config(
        config: GenericWebhookConfig,
        mode: GenericWebhookValidationMode,
    ) -> crate::Result<Self> {
        let normalized = normalize_config(config, mode)?;
        let client = build_http_client(normalized.timeout)?;
        Ok(Self {
            url: normalized.url,
            payload_field: normalized.payload_field,
            client,
            timeout: normalized.timeout,
            max_chars: normalized.max_chars,
            enforce_public_ip: normalized.enforce_public_ip,
        })
    }

    fn build_payload(event: &Event, payload_field: &str, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        serde_json::json!({ payload_field: text })
    }
}

fn normalize_config(
    config: GenericWebhookConfig,
    mode: GenericWebhookValidationMode,
) -> crate::Result<NormalizedGenericWebhookConfig> {
    let GenericWebhookConfig {
        url,
        payload_field,
        timeout,
        max_chars,
        enforce_public_ip,
        path_prefix,
        allowed_hosts,
    } = config;

    let payload_field = normalize_payload_field(payload_field)?;
    let allowed_hosts = normalize_allowed_hosts(allowed_hosts, mode)?;
    let path_prefix = path_prefix.and_then(normalize_optional_trimmed);

    validate_security_requirements(
        enforce_public_ip,
        &allowed_hosts,
        path_prefix.as_deref(),
        mode,
    )?;
    let path_prefix = validate_path_prefix(path_prefix, mode)?;
    let url = validate_target_url(&url, &allowed_hosts, path_prefix.as_deref())?;

    Ok(NormalizedGenericWebhookConfig {
        url,
        payload_field,
        timeout,
        max_chars,
        enforce_public_ip,
    })
}

fn normalize_payload_field(payload_field: String) -> crate::Result<String> {
    let payload_field = payload_field.trim();
    if payload_field.is_empty() {
        return Err(anyhow::anyhow!("generic webhook payload_field must not be empty").into());
    }
    Ok(payload_field.to_string())
}

fn validate_path_prefix(
    path_prefix: Option<String>,
    mode: GenericWebhookValidationMode,
) -> crate::Result<Option<String>> {
    let path_prefix = path_prefix.and_then(normalize_optional_trimmed);
    if mode == GenericWebhookValidationMode::Strict {
        let Some(path_prefix) = path_prefix else {
            return Err(anyhow::anyhow!("generic webhook strict mode requires path_prefix").into());
        };
        if !path_prefix.starts_with('/') {
            return Err(anyhow::anyhow!(
                "generic webhook strict mode requires path_prefix starting with '/'"
            )
            .into());
        }
        return Ok(Some(path_prefix));
    }
    Ok(path_prefix)
}

fn normalize_allowed_hosts(
    allowed_hosts: Vec<String>,
    mode: GenericWebhookValidationMode,
) -> crate::Result<Vec<String>> {
    if mode == GenericWebhookValidationMode::Strict
        && allowed_hosts.iter().any(|host| host.trim().is_empty())
    {
        return Err(anyhow::anyhow!("generic webhook allowed_hosts must not be empty").into());
    }
    Ok(normalize_nonempty_trimmed_vec(allowed_hosts))
}

fn validate_security_requirements(
    enforce_public_ip: bool,
    allowed_hosts: &[String],
    path_prefix: Option<&str>,
    mode: GenericWebhookValidationMode,
) -> crate::Result<()> {
    if !enforce_public_ip {
        if mode == GenericWebhookValidationMode::Strict {
            return Err(
                anyhow::anyhow!("generic webhook strict mode requires public ip check").into(),
            );
        }
        if allowed_hosts.is_empty() {
            return Err(anyhow::anyhow!(
                "generic webhook disabling public ip check requires allowed_hosts"
            )
            .into());
        }
    }

    if mode == GenericWebhookValidationMode::Strict {
        if allowed_hosts.is_empty() {
            return Err(
                anyhow::anyhow!("generic webhook strict mode requires allowed_hosts").into(),
            );
        }
        if path_prefix.is_none() {
            return Err(anyhow::anyhow!("generic webhook strict mode requires path_prefix").into());
        }
    }

    Ok(())
}

fn validate_target_url(
    url: &str,
    allowed_hosts: &[String],
    path_prefix: Option<&str>,
) -> crate::Result<reqwest::Url> {
    let url = parse_and_validate_https_url_basic(url)?;
    if let Some(prefix) = path_prefix {
        validate_url_path_prefix(&url, prefix)?;
    }

    if !allowed_hosts.is_empty() {
        let Some(host) = url.host_str() else {
            return Err(anyhow::anyhow!("url must have a host").into());
        };
        let allowed = allowed_hosts
            .iter()
            .any(|allowed_host| host.eq_ignore_ascii_case(allowed_host));
        if !allowed {
            return Err(anyhow::anyhow!("url host is not allowed").into());
        }
    }

    Ok(url)
}

fn normalize_optional_trimmed(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

fn normalize_nonempty_trimmed_vec(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(normalize_optional_trimmed)
        .collect()
}

impl Sink for GenericWebhookSink {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = select_http_client(
                &self.client,
                self.timeout,
                &self.url,
                self.enforce_public_ip,
            )
            .await?;

            let payload = Self::build_payload(event, &self.payload_field, self.max_chars);

            let resp = send_reqwest(
                client.post(self.url.as_str()).json(&payload),
                "generic webhook",
            )
            .await?;
            Ok(ensure_http_success(resp, "generic webhook").await?)
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

        let payload = GenericWebhookSink::build_payload(&event, "content", 16 * 1024);
        let text = payload["content"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_non_https_url() {
        let cfg = GenericWebhookConfig::new("http://example.com/webhook");
        let err = GenericWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_empty_payload_field() {
        let cfg = GenericWebhookConfig::new("https://example.com/webhook").with_payload_field(" ");
        let err = GenericWebhookSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("payload_field"), "{err:#}");
    }

    #[test]
    fn debug_redacts_url() {
        let cfg = GenericWebhookConfig::new("https://example.com/webhook?secret=x");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret=x"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");
    }

    #[test]
    fn disabling_public_ip_check_requires_allowed_hosts() {
        let cfg =
            GenericWebhookConfig::new("https://example.com/webhook").with_public_ip_check(false);
        let err = GenericWebhookSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("allowed_hosts"), "{err:#}");
    }

    #[test]
    fn strict_requires_allowed_hosts_and_path_prefix() {
        let cfg = GenericWebhookConfig::new("https://example.com/webhook");
        let err = GenericWebhookSink::new_strict(cfg).expect_err("expected strict validation");
        assert!(err.to_string().contains("allowed_hosts"), "{err:#}");

        let cfg = GenericWebhookConfig::new("https://example.com/webhook")
            .with_allowed_hosts(vec!["example.com".to_string()]);
        let err = GenericWebhookSink::new_strict(cfg).expect_err("expected strict validation");
        assert!(err.to_string().contains("path_prefix"), "{err:#}");
    }

    #[test]
    fn strict_rejects_disabled_public_ip_check() {
        let cfg = GenericWebhookConfig::new_strict(
            "https://example.com/hooks/notify",
            "/hooks/",
            vec!["example.com".to_string()],
        )
        .with_public_ip_check(false);
        let err = GenericWebhookSink::new_strict(cfg).expect_err("expected strict validation");
        assert!(err.to_string().contains("public ip"), "{err:#}");
    }

    #[test]
    fn strict_accepts_matching_host_and_path_prefix() {
        let cfg = GenericWebhookConfig::new_strict(
            "https://example.com/hooks/notify",
            "/hooks/",
            vec!["example.com".to_string()],
        )
        .with_payload_field("content");
        let sink = GenericWebhookSink::new_strict(cfg).expect("build strict sink");
        assert_eq!(sink.url.host_str().unwrap_or(""), "example.com");
        assert!(sink.url.path().starts_with("/hooks/"));
    }

    #[test]
    fn strict_rejects_unexpected_host() {
        let cfg = GenericWebhookConfig::new_strict(
            "https://evil.com/hooks/notify",
            "/hooks/",
            vec!["example.com".to_string()],
        );
        let err = GenericWebhookSink::new_strict(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn strict_rejects_unexpected_path() {
        let cfg = GenericWebhookConfig::new_strict(
            "https://example.com/api/notify",
            "/hooks/",
            vec!["example.com".to_string()],
        );
        let err = GenericWebhookSink::new_strict(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn trims_payload_field_allowed_hosts_and_path_prefix() {
        let cfg = GenericWebhookConfig::new("https://example.com/hooks/notify")
            .with_payload_field(" text ")
            .with_allowed_hosts(vec![" example.com ".to_string()])
            .with_path_prefix(" /hooks/ ");
        let sink = GenericWebhookSink::new(cfg).expect("build sink");
        assert_eq!(sink.payload_field, "text");
        assert_eq!(sink.url.host_str().unwrap_or(""), "example.com");
        assert!(sink.url.path().starts_with("/hooks/"));
    }
}
