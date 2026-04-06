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

#[derive(Debug, Clone, Copy, Default)]
pub struct StandardEnvHubOptions {
    pub default_sound_enabled: bool,
    pub require_sink: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnvTimeoutConfig {
    sink_timeout: Duration,
    hub_timeout: Duration,
}

const DEFAULT_SINK_TIMEOUT_MS: u64 = 5_000;
const MIN_HUB_TIMEOUT_SLACK_MS: u64 = 250;
const MAX_HUB_TIMEOUT_SLACK_MS: u64 = 1_000;

fn parse_bool_env_value(raw: &str) -> anyhow::Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("expected one of 1/0, true/false, yes/no, on/off"),
    }
}

fn env_bool<F>(key: &'static str, get: &F) -> anyhow::Result<Option<bool>>
where
    F: Fn(&str) -> Option<String>,
{
    get(key)
        .map(|value| parse_bool_env_value(&value).with_context(|| format!("invalid {key}")))
        .transpose()
}

fn env_nonempty<F>(key: &str, get: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_timeout_ms_env_optional<F>(key: &'static str, get: &F) -> anyhow::Result<Option<Duration>>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = env_nonempty(key, get) else {
        return Ok(None);
    };
    let timeout = value
        .parse::<u64>()
        .with_context(|| format!("invalid {key}"))?;
    Ok(Some(Duration::from_millis(timeout.max(1))))
}

fn resolve_timeout_ms_env<F>(
    primary_key: &'static str,
    fallback_key: &'static str,
    get: &F,
) -> anyhow::Result<Duration>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(timeout) = parse_timeout_ms_env_optional(primary_key, get)? {
        return Ok(timeout);
    }
    if let Some(timeout) = parse_timeout_ms_env_optional(fallback_key, get)? {
        return Ok(timeout);
    }
    Ok(Duration::from_millis(DEFAULT_SINK_TIMEOUT_MS))
}

fn hub_timeout_with_slack(sink_timeout: Duration) -> Duration {
    let sink_timeout_ms = sink_timeout.as_millis() as u64;
    let slack_ms = (sink_timeout_ms / 5).clamp(MIN_HUB_TIMEOUT_SLACK_MS, MAX_HUB_TIMEOUT_SLACK_MS);
    sink_timeout.saturating_add(Duration::from_millis(slack_ms))
}

fn parse_timeout_config<F>(get: &F) -> anyhow::Result<EnvTimeoutConfig>
where
    F: Fn(&str) -> Option<String>,
{
    const NOTIFY_TIMEOUT_MS_ENV: &str = "NOTIFY_TIMEOUT_MS";
    const NOTIFY_SINK_TIMEOUT_MS_ENV: &str = "NOTIFY_SINK_TIMEOUT_MS";
    const NOTIFY_HUB_TIMEOUT_MS_ENV: &str = "NOTIFY_HUB_TIMEOUT_MS";

    let sink_timeout =
        resolve_timeout_ms_env(NOTIFY_SINK_TIMEOUT_MS_ENV, NOTIFY_TIMEOUT_MS_ENV, get)?;
    let hub_timeout = match parse_timeout_ms_env_optional(NOTIFY_HUB_TIMEOUT_MS_ENV, get)? {
        Some(timeout) => timeout,
        None => hub_timeout_with_slack(sink_timeout),
    };

    if hub_timeout <= sink_timeout {
        anyhow::bail!(
            "{NOTIFY_HUB_TIMEOUT_MS_ENV} must be greater than the effective sink timeout; keep slack so Hub does not time out before the sink finishes"
        );
    }

    Ok(EnvTimeoutConfig {
        sink_timeout,
        hub_timeout,
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
    const NOTIFY_EVENTS_ENV: &str = "NOTIFY_EVENTS";

    let sound_enabled = env_bool(NOTIFY_SOUND_ENV, get)?.unwrap_or(options.default_sound_enabled);
    let timeouts = parse_timeout_config(get)?;

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
    fn invalid_boolean_env_fails_closed() {
        let env = HashMap::from([(String::from("NOTIFY_SOUND"), String::from("maybe"))]);

        let err = match build_hub_from_env(StandardEnvHubOptions::default(), &|key| {
            env.get(key).cloned()
        }) {
            Ok(_) => panic!("invalid bool should fail"),
            Err(err) => err,
        };

        let msg = format!("{err:#}");
        assert!(msg.contains("invalid NOTIFY_SOUND"), "{msg}");
        assert!(msg.contains("expected one of"), "{msg}");
    }

    #[test]
    fn parse_timeout_config_uses_legacy_timeout_for_sink_and_adds_hub_slack() {
        let env = HashMap::from([(String::from("NOTIFY_TIMEOUT_MS"), String::from("1200"))]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(1200),
                hub_timeout: Duration::from_millis(1450),
            }
        );
    }

    #[test]
    fn parse_timeout_config_supports_separate_sink_and_hub_timeouts() {
        let env = HashMap::from([
            (String::from("NOTIFY_SINK_TIMEOUT_MS"), String::from("1200")),
            (String::from("NOTIFY_HUB_TIMEOUT_MS"), String::from("3400")),
        ]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(1200),
                hub_timeout: Duration::from_millis(3400),
            }
        );
    }

    #[test]
    fn parse_timeout_config_prefers_explicit_timeouts_over_legacy_fallback() {
        let env = HashMap::from([
            (String::from("NOTIFY_TIMEOUT_MS"), String::from("4700")),
            (String::from("NOTIFY_SINK_TIMEOUT_MS"), String::from("1200")),
            (String::from("NOTIFY_HUB_TIMEOUT_MS"), String::from("3400")),
        ]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(1200),
                hub_timeout: Duration::from_millis(3400),
            }
        );
    }

    #[test]
    fn parse_timeout_config_ignores_invalid_legacy_timeout_when_explicit_values_exist() {
        let env = HashMap::from([
            (String::from("NOTIFY_TIMEOUT_MS"), String::from("oops")),
            (String::from("NOTIFY_SINK_TIMEOUT_MS"), String::from("1200")),
            (String::from("NOTIFY_HUB_TIMEOUT_MS"), String::from("3400")),
        ]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(1200),
                hub_timeout: Duration::from_millis(3400),
            }
        );
    }

    #[test]
    fn parse_timeout_config_reports_invalid_explicit_timeout_key() {
        let env = HashMap::from([(String::from("NOTIFY_HUB_TIMEOUT_MS"), String::from("oops"))]);

        let err = parse_timeout_config(&|key| env.get(key).cloned()).expect_err("invalid timeout");

        let msg = format!("{err:#}");
        assert!(msg.contains("invalid NOTIFY_HUB_TIMEOUT_MS"), "{msg}");
    }

    #[test]
    fn parse_timeout_config_uses_default_hub_slack_when_only_sink_timeout_is_explicit() {
        let env = HashMap::from([(String::from("NOTIFY_SINK_TIMEOUT_MS"), String::from("1200"))]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(1200),
                hub_timeout: Duration::from_millis(1450),
            }
        );
    }

    #[test]
    fn parse_timeout_config_clamps_default_hub_slack_for_large_timeouts() {
        let env = HashMap::from([(
            String::from("NOTIFY_SINK_TIMEOUT_MS"),
            String::from("20000"),
        )]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(20_000),
                hub_timeout: Duration::from_millis(21_000),
            }
        );
    }

    #[test]
    fn parse_timeout_config_rejects_hub_timeout_without_slack() {
        let env = HashMap::from([
            (String::from("NOTIFY_SINK_TIMEOUT_MS"), String::from("1200")),
            (String::from("NOTIFY_HUB_TIMEOUT_MS"), String::from("1200")),
        ]);

        let err = parse_timeout_config(&|key| env.get(key).cloned())
            .expect_err("hub timeout should leave some slack");

        let msg = format!("{err:#}");
        assert!(msg.contains("NOTIFY_HUB_TIMEOUT_MS"), "{msg}");
        assert!(
            msg.contains("greater than the effective sink timeout"),
            "{msg}"
        );
    }
}
