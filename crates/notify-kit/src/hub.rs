use std::collections::{BTreeSet, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use crate::sinks::Sink;

mod backpressure;
mod fanout;
mod runtime;
#[cfg(test)]
mod tests;

pub(super) const DEFAULT_MAX_INFLIGHT_EVENTS: usize = 128;
pub(super) const DEFAULT_MAX_SINK_SENDS_IN_PARALLEL: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryNotifyError {
    NoTokioRuntime,
    Overloaded,
}

impl std::fmt::Display for TryNotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTokioRuntime => write!(f, "no tokio runtime or time driver"),
            Self::Overloaded => write!(f, "hub is overloaded"),
        }
    }
}

impl std::error::Error for TryNotifyError {}

#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Optional allow-list for event kinds.
    ///
    /// - `None`: allow all event kinds.
    /// - `Some(set)`: only allow event kinds present in the set.
    pub enabled_kinds: Option<BTreeSet<String>>,
    /// Per-sink timeout to ensure notifications never block the caller.
    ///
    /// This is a **hard upper bound** enforced by `Hub` (via `tokio::time::timeout`) around each
    /// `Sink::send`. If a sink has its own internal timeout (e.g. an HTTP request timeout), keep
    /// `per_sink_timeout` >= that value (and ideally leave some slack for preflight work like DNS
    /// checks), otherwise `Hub` may time out first. Calling `Hub::send` or `Hub::notify` with
    /// sinks configured therefore requires a Tokio runtime with the time driver enabled.
    pub per_sink_timeout: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            enabled_kinds: None,
            per_sink_timeout: Duration::from_secs(5),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubLimits {
    /// Maximum number of events that may be in-flight inside `Hub`.
    ///
    /// This applies to both `notify()` background tasks and `send().await` calls waiting on sink
    /// fan-out.
    pub max_inflight_events: usize,
    /// Maximum number of sink sends that may run in parallel for a single event fan-out.
    pub max_sink_sends_in_parallel: usize,
}

impl Default for HubLimits {
    fn default() -> Self {
        Self {
            max_inflight_events: DEFAULT_MAX_INFLIGHT_EVENTS,
            max_sink_sends_in_parallel: DEFAULT_MAX_SINK_SENDS_IN_PARALLEL,
        }
    }
}

impl HubLimits {
    #[must_use]
    pub fn with_max_inflight_events(mut self, max_inflight_events: usize) -> Self {
        self.max_inflight_events = max_inflight_events.max(1);
        self
    }

    #[must_use]
    pub fn with_max_sink_sends_in_parallel(mut self, max_sink_sends_in_parallel: usize) -> Self {
        self.max_sink_sends_in_parallel = max_sink_sends_in_parallel.max(1);
        self
    }
}

#[derive(Clone)]
pub struct Hub {
    pub(super) inner: Arc<HubInner>,
}

pub(super) struct HubInner {
    pub(super) enabled_kinds: Option<HashSet<String>>,
    pub(super) sinks: Vec<HubSink>,
    pub(super) per_sink_timeout: Duration,
    pub(super) inflight: Arc<tokio::sync::Semaphore>,
    pub(super) max_sink_sends_in_parallel: usize,
}

pub(super) struct HubSink {
    pub(super) sink: Arc<dyn Sink>,
    pub(super) name: Option<&'static str>,
}

impl Hub {
    pub fn new(config: HubConfig, sinks: Vec<Arc<dyn Sink>>) -> Self {
        Self::new_with_limits(config, sinks, HubLimits::default())
    }

    pub fn new_with_inflight_limit(
        config: HubConfig,
        sinks: Vec<Arc<dyn Sink>>,
        max_inflight_events: usize,
    ) -> Self {
        Self::new_with_limits(
            config,
            sinks,
            HubLimits::default().with_max_inflight_events(max_inflight_events),
        )
    }

    pub fn new_with_limits(
        config: HubConfig,
        sinks: Vec<Arc<dyn Sink>>,
        limits: HubLimits,
    ) -> Self {
        let limits = HubLimits::default()
            .with_max_inflight_events(limits.max_inflight_events)
            .with_max_sink_sends_in_parallel(limits.max_sink_sends_in_parallel);
        let sinks = sinks
            .into_iter()
            .map(|sink| HubSink {
                name: std::panic::catch_unwind(AssertUnwindSafe(|| sink.name())).ok(),
                sink,
            })
            .collect();
        let inner = HubInner {
            enabled_kinds: config
                .enabled_kinds
                .map(|enabled_kinds| enabled_kinds.into_iter().collect()),
            sinks,
            per_sink_timeout: config.per_sink_timeout,
            inflight: Arc::new(tokio::sync::Semaphore::new(limits.max_inflight_events)),
            max_sink_sends_in_parallel: limits.max_sink_sends_in_parallel,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub(super) fn is_kind_enabled(&self, kind: &str) -> bool {
        let Some(enabled) = &self.inner.enabled_kinds else {
            return true;
        };
        enabled.contains(kind)
    }
}
