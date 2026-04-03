//! Convenience helpers for bootstrapping a [`crate::Hub`] from a small env convention.
//!
//! This module is intentionally not part of the core notification abstraction. Prefer your own
//! integration layer when you need project-specific env/CLI/file configuration semantics.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::{
    FeishuWebhookConfig, FeishuWebhookSink, GenericWebhookConfig, GenericWebhookSink, Hub,
    HubConfig, HubLimits, Sink, SlackWebhookConfig, SlackWebhookSink, SoundConfig, SoundSink,
};

const DEFAULT_NOTIFY_TIMEOUT_MS: u64 = 5000;
const MIN_HUB_TIMEOUT_SLACK_MS: u64 = 250;
const MAX_HUB_TIMEOUT_SLACK_MS: u64 = 1000;

#[derive(Debug, Clone, Copy, Default)]
pub struct StandardEnvHubOptions {
    pub default_sound_enabled: bool,
    pub require_sink: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimeoutConfig {
    sink_timeout: Duration,
    hub_timeout: Duration,
}

fn parse_bool_env_value(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn env_bool<F>(key: &str, get: &F) -> Option<bool>
where
    F: Fn(&str) -> Option<String>,
{
    get(key).and_then(|value| parse_bool_env_value(&value))
}

fn env_nonempty<F>(key: &str, get: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn hub_timeout_slack(timeout: Duration) -> Duration {
    let timeout_ms = timeout.as_millis();
    let proportional_slack_ms = (timeout_ms / 5) as u64;
    Duration::from_millis(
        proportional_slack_ms.clamp(MIN_HUB_TIMEOUT_SLACK_MS, MAX_HUB_TIMEOUT_SLACK_MS),
    )
}

fn parse_timeout_ms_env<F>(key: &str, get: &F) -> anyhow::Result<TimeoutConfig>
where
    F: Fn(&str) -> Option<String>,
{
    let timeout = env_nonempty(key, get)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(DEFAULT_NOTIFY_TIMEOUT_MS)
        .max(1);
    let sink_timeout = Duration::from_millis(timeout);
    Ok(TimeoutConfig {
        sink_timeout,
        hub_timeout: sink_timeout.saturating_add(hub_timeout_slack(sink_timeout)),
    })
}

pub fn build_hub_from_standard_env(options: StandardEnvHubOptions) -> anyhow::Result<Option<Hub>> {
    build_hub_from_env(options, &|key| std::env::var(key).ok())
}

fn build_hub_from_env<F>(options: StandardEnvHubOptions, get: &F) -> anyhow::Result<Option<Hub>>
where
    F: Fn(&str) -> Option<String>,
{
    const NOTIFY_SOUND_ENV: &str = "NOTIFY_SOUND";
    const NOTIFY_WEBHOOK_URL_ENV: &str = "NOTIFY_WEBHOOK_URL";
    const NOTIFY_WEBHOOK_FIELD_ENV: &str = "NOTIFY_WEBHOOK_FIELD";
    const NOTIFY_FEISHU_WEBHOOK_URL_ENV: &str = "NOTIFY_FEISHU_WEBHOOK_URL";
    const NOTIFY_SLACK_WEBHOOK_URL_ENV: &str = "NOTIFY_SLACK_WEBHOOK_URL";
    const NOTIFY_TIMEOUT_MS_ENV: &str = "NOTIFY_TIMEOUT_MS";
    const NOTIFY_EVENTS_ENV: &str = "NOTIFY_EVENTS";

    let sound_enabled = env_bool(NOTIFY_SOUND_ENV, get).unwrap_or(options.default_sound_enabled);
    let timeouts = parse_timeout_ms_env(NOTIFY_TIMEOUT_MS_ENV, get)
        .with_context(|| format!("invalid {NOTIFY_TIMEOUT_MS_ENV}"))?;

    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
    if sound_enabled {
        sinks.push(Arc::new(SoundSink::new(SoundConfig { command_argv: None })));
    }

    if let Some(url) = env_nonempty(NOTIFY_WEBHOOK_URL_ENV, get) {
        let mut cfg = GenericWebhookConfig::new(url).with_timeout(timeouts.sink_timeout);
        if let Some(field) = env_nonempty(NOTIFY_WEBHOOK_FIELD_ENV, get) {
            cfg = cfg.with_payload_field(field);
        }
        sinks.push(Arc::new(
            GenericWebhookSink::new(cfg).context("build generic webhook sink")?,
        ));
    }

    if let Some(url) = env_nonempty(NOTIFY_FEISHU_WEBHOOK_URL_ENV, get) {
        let cfg = FeishuWebhookConfig::new(url).with_timeout(timeouts.sink_timeout);
        sinks.push(Arc::new(
            FeishuWebhookSink::new(cfg).context("build feishu sink")?,
        ));
    }

    if let Some(url) = env_nonempty(NOTIFY_SLACK_WEBHOOK_URL_ENV, get) {
        let cfg = SlackWebhookConfig::new(url).with_timeout(timeouts.sink_timeout);
        sinks.push(Arc::new(
            SlackWebhookSink::new(cfg).context("build slack sink")?,
        ));
    }

    if sinks.is_empty() {
        if options.require_sink {
            anyhow::bail!(
                "no notification sinks configured (enable {NOTIFY_SOUND_ENV}=1 or provide webhook envs)"
            );
        }
        return Ok(None);
    }

    let enabled_kinds = get(NOTIFY_EVENTS_ENV).and_then(|raw| {
        let set = raw
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        if set.is_empty() { None } else { Some(set) }
    });

    Ok(Some(Hub::new_with_limits(
        HubConfig {
            enabled_kinds,
            per_sink_timeout: timeouts.hub_timeout,
        },
        sinks,
        HubLimits::default(),
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn build_hub_from_standard_env_uses_sound_when_enabled() {
        let env = HashMap::from([(String::from("NOTIFY_SOUND"), String::from("1"))]);

        let hub = build_hub_from_env(StandardEnvHubOptions::default(), &|key| {
            env.get(key).cloned()
        })
        .expect("build hub")
        .expect("hub present");
        assert_eq!(
            hub.try_notify(crate::Event::new("kind", crate::Severity::Info, "title")),
            Err(crate::TryNotifyError::NoTokioRuntime)
        );
    }

    #[test]
    fn build_hub_from_standard_env_respects_require_sink() {
        let env = HashMap::<String, String>::new();

        let result = build_hub_from_env(
            StandardEnvHubOptions {
                default_sound_enabled: false,
                require_sink: true,
            },
            &|key| env.get(key).cloned(),
        );
        let err = match result {
            Ok(_) => panic!("expected error, got success"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("no notification sinks configured"));
    }

    #[test]
    fn parse_timeout_ms_env_uses_default_timeout_and_hub_slack() {
        let env = HashMap::<String, String>::new();

        let cfg = parse_timeout_ms_env("NOTIFY_TIMEOUT_MS", &|key| env.get(key).cloned())
            .expect("parse timeout");
        assert_eq!(
            cfg,
            TimeoutConfig {
                sink_timeout: Duration::from_secs(5),
                hub_timeout: Duration::from_secs(6),
            }
        );
    }

    #[test]
    fn parse_timeout_ms_env_clamps_hub_slack_for_small_values() {
        let env = HashMap::from([(String::from("NOTIFY_TIMEOUT_MS"), String::from("2000"))]);

        let cfg = parse_timeout_ms_env("NOTIFY_TIMEOUT_MS", &|key| env.get(key).cloned())
            .expect("parse timeout");
        assert_eq!(cfg.sink_timeout, Duration::from_millis(2000));
        assert_eq!(cfg.hub_timeout, Duration::from_millis(2400));
    }
}
