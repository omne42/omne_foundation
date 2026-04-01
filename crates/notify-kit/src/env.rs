//! Convenience helpers for bootstrapping a [`crate::Hub`] from a small env convention.
//!
//! This module is intentionally not part of the core notification abstraction. Prefer your own
//! integration layer when you need project-specific env/CLI/file configuration semantics.

use std::collections::BTreeSet;
use std::num::ParseIntError;
use std::sync::Arc;
use std::time::Duration;

#[cfg(any(feature = "all-sinks", feature = "feishu"))]
use crate::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
use crate::{GenericWebhookConfig, GenericWebhookSink};
use crate::{Hub, HubConfig, HubLimits, Sink};
#[cfg(any(feature = "all-sinks", feature = "slack"))]
use crate::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "sound"))]
use crate::{SoundConfig, SoundSink};

const DEFAULT_TIMEOUT_MS: u64 = 5000;
const LEGACY_HUB_TIMEOUT_GRACE_MIN_MS: u64 = 250;
const LEGACY_HUB_TIMEOUT_GRACE_MAX_MS: u64 = 1000;

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

#[derive(Debug)]
pub enum EnvHubError {
    InvalidBoolean {
        key: &'static str,
        value: String,
    },
    InvalidTimeoutMs {
        key: &'static str,
        value: String,
        source: ParseIntError,
    },
    SinkFeatureUnavailable {
        env_var: &'static str,
        feature: &'static str,
    },
    SinkBuild {
        sink: &'static str,
        source: crate::Error,
    },
    NoSinksConfigured {
        env_vars: &'static [&'static str],
    },
}

impl std::fmt::Display for EnvHubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBoolean { key, value } => write!(
                f,
                "invalid {key}={value:?}: expected one of 1/0, true/false, yes/no, on/off"
            ),
            Self::InvalidTimeoutMs { key, value, source } => {
                write!(f, "invalid {key}={value:?}: {source}")
            }
            Self::SinkFeatureUnavailable { env_var, feature } => write!(
                f,
                "{env_var} requires notify-kit feature `{feature}` when `all-sinks` is disabled"
            ),
            Self::SinkBuild { sink, source } => write!(f, "build {sink} sink: {source}"),
            Self::NoSinksConfigured { env_vars } => write!(
                f,
                "no notification sinks configured (enable {} or provide webhook envs)",
                env_vars.join(" / ")
            ),
        }
    }
}

impl std::error::Error for EnvHubError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidTimeoutMs { source, .. } => Some(source),
            Self::SinkBuild { source, .. } => Some(source),
            Self::InvalidBoolean { .. }
            | Self::SinkFeatureUnavailable { .. }
            | Self::NoSinksConfigured { .. } => None,
        }
    }
}

fn parse_bool_env_value(key: &'static str, raw: &str) -> Result<bool, EnvHubError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(EnvHubError::InvalidBoolean {
            key,
            value: raw.to_string(),
        }),
    }
}

fn env_bool<F>(key: &'static str, get: &F) -> Result<Option<bool>, EnvHubError>
where
    F: Fn(&str) -> Option<String>,
{
    get(key)
        .map(|value| parse_bool_env_value(key, &value))
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

fn parse_timeout_ms_env_optional<F>(
    key: &'static str,
    get: &F,
) -> Result<Option<Duration>, EnvHubError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = env_nonempty(key, get) else {
        return Ok(None);
    };
    let timeout = value
        .parse::<u64>()
        .map_err(|source| EnvHubError::InvalidTimeoutMs { key, value, source })?;
    Ok(Some(Duration::from_millis(timeout.max(1))))
}

fn parse_timeout_config<F>(get: &F) -> Result<EnvTimeoutConfig, EnvHubError>
where
    F: Fn(&str) -> Option<String>,
{
    const NOTIFY_TIMEOUT_MS_ENV: &str = "NOTIFY_TIMEOUT_MS";
    const NOTIFY_SINK_TIMEOUT_MS_ENV: &str = "NOTIFY_SINK_TIMEOUT_MS";
    const NOTIFY_HUB_TIMEOUT_MS_ENV: &str = "NOTIFY_HUB_TIMEOUT_MS";

    let explicit_sink_timeout = parse_timeout_ms_env_optional(NOTIFY_SINK_TIMEOUT_MS_ENV, get)?;
    let explicit_hub_timeout = parse_timeout_ms_env_optional(NOTIFY_HUB_TIMEOUT_MS_ENV, get)?;

    if let (Some(sink_timeout), Some(hub_timeout)) = (explicit_sink_timeout, explicit_hub_timeout) {
        return Ok(EnvTimeoutConfig {
            sink_timeout,
            hub_timeout,
        });
    }

    if let Some(legacy_timeout) = parse_timeout_ms_env_optional(NOTIFY_TIMEOUT_MS_ENV, get)? {
        let sink_timeout = explicit_sink_timeout.unwrap_or(legacy_timeout);
        let hub_timeout = explicit_hub_timeout
            .unwrap_or_else(|| sink_timeout.saturating_add(legacy_hub_timeout_grace(sink_timeout)));
        return Ok(EnvTimeoutConfig {
            sink_timeout,
            hub_timeout,
        });
    }

    Ok(EnvTimeoutConfig {
        sink_timeout: explicit_sink_timeout
            .unwrap_or_else(|| Duration::from_millis(DEFAULT_TIMEOUT_MS)),
        hub_timeout: explicit_hub_timeout
            .unwrap_or_else(|| Duration::from_millis(DEFAULT_TIMEOUT_MS)),
    })
}

fn legacy_hub_timeout_grace(sink_timeout: Duration) -> Duration {
    let grace_ms = sink_timeout.as_millis().saturating_div(5).clamp(
        u128::from(LEGACY_HUB_TIMEOUT_GRACE_MIN_MS),
        u128::from(LEGACY_HUB_TIMEOUT_GRACE_MAX_MS),
    );
    Duration::from_millis(
        u64::try_from(grace_ms).expect("legacy hub timeout grace should stay within u64"),
    )
}

#[cfg(not(all(
    feature = "sound",
    feature = "generic-webhook",
    feature = "feishu",
    feature = "slack"
)))]
#[allow(dead_code)]
fn unavailable_sink_feature_error(env_var: &'static str, feature: &'static str) -> EnvHubError {
    EnvHubError::SinkFeatureUnavailable { env_var, feature }
}

pub fn build_hub_from_standard_env(
    options: StandardEnvHubOptions,
) -> Result<Option<Hub>, EnvHubError> {
    build_hub_from_env(options, &|key| std::env::var(key).ok())
}

fn build_hub_from_env<F>(
    options: StandardEnvHubOptions,
    get: &F,
) -> Result<Option<Hub>, EnvHubError>
where
    F: Fn(&str) -> Option<String>,
{
    const NOTIFY_SOUND_ENV: &str = "NOTIFY_SOUND";
    const NOTIFY_WEBHOOK_URL_ENV: &str = "NOTIFY_WEBHOOK_URL";
    #[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
    const NOTIFY_WEBHOOK_FIELD_ENV: &str = "NOTIFY_WEBHOOK_FIELD";
    const NOTIFY_FEISHU_WEBHOOK_URL_ENV: &str = "NOTIFY_FEISHU_WEBHOOK_URL";
    const NOTIFY_SLACK_WEBHOOK_URL_ENV: &str = "NOTIFY_SLACK_WEBHOOK_URL";
    const NOTIFY_EVENTS_ENV: &str = "NOTIFY_EVENTS";
    const NOTIFY_REQUIRED_ENV_VARS: &[&str] = &[
        NOTIFY_SOUND_ENV,
        NOTIFY_WEBHOOK_URL_ENV,
        NOTIFY_FEISHU_WEBHOOK_URL_ENV,
        NOTIFY_SLACK_WEBHOOK_URL_ENV,
    ];

    let sound_enabled = env_bool(NOTIFY_SOUND_ENV, get)?.unwrap_or(options.default_sound_enabled);
    let timeouts = parse_timeout_config(get)?;

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
        let mut cfg = GenericWebhookConfig::new(url).with_timeout(timeouts.sink_timeout);
        if let Some(field) = env_nonempty(NOTIFY_WEBHOOK_FIELD_ENV, get) {
            cfg = cfg.with_payload_field(field);
        }
        sinks.push(Arc::new(GenericWebhookSink::new(cfg).map_err(
            |source| EnvHubError::SinkBuild {
                sink: "generic webhook",
                source,
            },
        )?));
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
        let cfg = FeishuWebhookConfig::new(url).with_timeout(timeouts.sink_timeout);
        sinks.push(Arc::new(FeishuWebhookSink::new(cfg).map_err(|source| {
            EnvHubError::SinkBuild {
                sink: "feishu",
                source,
            }
        })?));
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
        let cfg = SlackWebhookConfig::new(url).with_timeout(timeouts.sink_timeout);
        sinks.push(Arc::new(SlackWebhookSink::new(cfg).map_err(|source| {
            EnvHubError::SinkBuild {
                sink: "slack",
                source,
            }
        })?));
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
            return Err(EnvHubError::NoSinksConfigured {
                env_vars: NOTIFY_REQUIRED_ENV_VARS,
            });
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

    #[test]
    fn build_hub_from_standard_env_rejects_invalid_bool() {
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
    fn parse_timeout_config_uses_legacy_timeout_as_sink_timeout_and_adds_hub_slack() {
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
    fn parse_timeout_config_uses_legacy_timeout_for_missing_explicit_hub_timeout() {
        let env = HashMap::from([
            (String::from("NOTIFY_TIMEOUT_MS"), String::from("1200")),
            (String::from("NOTIFY_SINK_TIMEOUT_MS"), String::from("900")),
        ]);

        let config = parse_timeout_config(&|key| env.get(key).cloned()).expect("parse timeout");

        assert_eq!(
            config,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(900),
                hub_timeout: Duration::from_millis(1150),
            }
        );
    }

    #[test]
    fn parse_timeout_config_clamps_legacy_hub_timeout_grace() {
        let fast_env = HashMap::from([(String::from("NOTIFY_TIMEOUT_MS"), String::from("1"))]);
        let fast = parse_timeout_config(&|key| fast_env.get(key).cloned()).expect("fast timeout");
        assert_eq!(
            fast,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(1),
                hub_timeout: Duration::from_millis(251),
            }
        );

        let slow_env = HashMap::from([(String::from("NOTIFY_TIMEOUT_MS"), String::from("9000"))]);
        let slow = parse_timeout_config(&|key| slow_env.get(key).cloned()).expect("slow timeout");
        assert_eq!(
            slow,
            EnvTimeoutConfig {
                sink_timeout: Duration::from_millis(9000),
                hub_timeout: Duration::from_millis(10000),
            }
        );
    }
}
