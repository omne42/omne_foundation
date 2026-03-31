//! Convenience helpers for bootstrapping a [`crate::Hub`] from a small env convention.
//!
//! This module is intentionally not part of the core notification abstraction. Prefer your own
//! integration layer when you need project-specific env/CLI/file configuration semantics.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

#[cfg(any(feature = "all-sinks", feature = "feishu"))]
use crate::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
use crate::{GenericWebhookConfig, GenericWebhookSink};
use crate::{Hub, HubConfig, HubLimits, Sink};
#[cfg(any(feature = "all-sinks", feature = "slack"))]
use crate::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "sound"))]
use crate::{SoundConfig, SoundSink};

#[derive(Debug, Clone, Copy, Default)]
pub struct StandardEnvHubOptions {
    pub default_sound_enabled: bool,
    pub require_sink: bool,
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

fn parse_timeout_ms_env<F>(key: &str, get: &F) -> anyhow::Result<Duration>
where
    F: Fn(&str) -> Option<String>,
{
    let timeout = env_nonempty(key, get)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(5000);
    Ok(Duration::from_millis(timeout.max(1)))
}

#[cfg(not(all(
    feature = "sound",
    feature = "generic-webhook",
    feature = "feishu",
    feature = "slack"
)))]
#[allow(dead_code)]
fn unavailable_sink_feature_error(env_var: &str, feature: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{env_var} requires notify-kit feature `{feature}` when `all-sinks` is disabled"
    )
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
    #[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
    const NOTIFY_WEBHOOK_FIELD_ENV: &str = "NOTIFY_WEBHOOK_FIELD";
    const NOTIFY_FEISHU_WEBHOOK_URL_ENV: &str = "NOTIFY_FEISHU_WEBHOOK_URL";
    const NOTIFY_SLACK_WEBHOOK_URL_ENV: &str = "NOTIFY_SLACK_WEBHOOK_URL";
    const NOTIFY_TIMEOUT_MS_ENV: &str = "NOTIFY_TIMEOUT_MS";
    const NOTIFY_EVENTS_ENV: &str = "NOTIFY_EVENTS";

    let sound_enabled = env_bool(NOTIFY_SOUND_ENV, get).unwrap_or(options.default_sound_enabled);
    let timeout = parse_timeout_ms_env(NOTIFY_TIMEOUT_MS_ENV, get)
        .with_context(|| format!("invalid {NOTIFY_TIMEOUT_MS_ENV}"))?;

    #[cfg(any(
        feature = "all-sinks",
        feature = "sound",
        feature = "generic-webhook",
        feature = "feishu",
        feature = "slack"
    ))]
    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
    #[cfg(all(
        not(feature = "all-sinks"),
        not(any(
            feature = "sound",
            feature = "generic-webhook",
            feature = "feishu",
            feature = "slack"
        ))
    ))]
    let sinks: Vec<Arc<dyn Sink>> = Vec::new();
    if sound_enabled {
        #[cfg(any(feature = "all-sinks", feature = "sound"))]
        sinks.push(Arc::new(SoundSink::new(SoundConfig { command_argv: None })));
        #[cfg(all(not(feature = "all-sinks"), not(feature = "sound")))]
        return Err(unavailable_sink_feature_error(NOTIFY_SOUND_ENV, "sound"));
    }

    #[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
    if let Some(url) = env_nonempty(NOTIFY_WEBHOOK_URL_ENV, get) {
        let mut cfg = GenericWebhookConfig::new(url).with_timeout(timeout);
        if let Some(field) = env_nonempty(NOTIFY_WEBHOOK_FIELD_ENV, get) {
            cfg = cfg.with_payload_field(field);
        }
        sinks.push(Arc::new(
            GenericWebhookSink::new(cfg).context("build generic webhook sink")?,
        ));
    }
    #[cfg(all(not(feature = "all-sinks"), not(feature = "generic-webhook")))]
    if env_nonempty(NOTIFY_WEBHOOK_URL_ENV, get).is_some() {
        return Err(unavailable_sink_feature_error(
            NOTIFY_WEBHOOK_URL_ENV,
            "generic-webhook",
        ));
    }

    #[cfg(any(feature = "all-sinks", feature = "feishu"))]
    if let Some(url) = env_nonempty(NOTIFY_FEISHU_WEBHOOK_URL_ENV, get) {
        let cfg = FeishuWebhookConfig::new(url).with_timeout(timeout);
        sinks.push(Arc::new(
            FeishuWebhookSink::new(cfg).context("build feishu sink")?,
        ));
    }
    #[cfg(all(not(feature = "all-sinks"), not(feature = "feishu")))]
    if env_nonempty(NOTIFY_FEISHU_WEBHOOK_URL_ENV, get).is_some() {
        return Err(unavailable_sink_feature_error(
            NOTIFY_FEISHU_WEBHOOK_URL_ENV,
            "feishu",
        ));
    }

    #[cfg(any(feature = "all-sinks", feature = "slack"))]
    if let Some(url) = env_nonempty(NOTIFY_SLACK_WEBHOOK_URL_ENV, get) {
        let cfg = SlackWebhookConfig::new(url).with_timeout(timeout);
        sinks.push(Arc::new(
            SlackWebhookSink::new(cfg).context("build slack sink")?,
        ));
    }
    #[cfg(all(not(feature = "all-sinks"), not(feature = "slack")))]
    if env_nonempty(NOTIFY_SLACK_WEBHOOK_URL_ENV, get).is_some() {
        return Err(unavailable_sink_feature_error(
            NOTIFY_SLACK_WEBHOOK_URL_ENV,
            "slack",
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
            per_sink_timeout: timeout,
        },
        sinks,
        HubLimits::default(),
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[cfg(any(feature = "all-sinks", feature = "sound"))]
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

    #[cfg(all(not(feature = "all-sinks"), not(feature = "sound")))]
    #[test]
    fn build_hub_from_standard_env_rejects_unavailable_sound_sink() {
        let env = HashMap::from([(String::from("NOTIFY_SOUND"), String::from("1"))]);

        let err = match build_hub_from_env(StandardEnvHubOptions::default(), &|key| {
            env.get(key).cloned()
        }) {
            Ok(_) => panic!("expected error, got success"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("NOTIFY_SOUND"), "{err:#}");
        assert!(err.to_string().contains("feature `sound`"), "{err:#}");
    }

    #[cfg(all(not(feature = "all-sinks"), not(feature = "slack")))]
    #[test]
    fn build_hub_from_standard_env_rejects_unavailable_slack_sink() {
        let env = HashMap::from([(
            String::from("NOTIFY_SLACK_WEBHOOK_URL"),
            String::from("https://hooks.slack.com/services/test"),
        )]);

        let err = match build_hub_from_env(StandardEnvHubOptions::default(), &|key| {
            env.get(key).cloned()
        }) {
            Ok(_) => panic!("expected error, got success"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("NOTIFY_SLACK_WEBHOOK_URL"),
            "{err:#}"
        );
        assert!(err.to_string().contains("feature `slack`"), "{err:#}");
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
}
