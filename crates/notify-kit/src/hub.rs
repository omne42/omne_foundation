use std::collections::{BTreeSet, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::stream::{FuturesUnordered, StreamExt};

use crate::error::SinkFailure;
use crate::event::Event;
use crate::log::{warn_hub_notify_dropped, warn_hub_notify_failed};
use crate::sinks::Sink;

const DEFAULT_MAX_INFLIGHT_EVENTS: usize = 128;
const DEFAULT_MAX_SINK_SENDS_IN_PARALLEL: usize = 16;

fn ensure_tokio_time_driver(operation: &'static str) -> crate::Result<()> {
    std::panic::catch_unwind(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    })
    .map_err(|_| anyhow::anyhow!("tokio runtime time driver is required for {operation}").into())
}

fn has_tokio_time_driver() -> bool {
    std::panic::catch_unwind(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    })
    .is_ok()
}

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
    /// checks), otherwise `Hub` may time out first. Calling `Hub::send`,
    /// `Hub::notify_best_effort`, or `Hub::notify` with sinks configured therefore requires a
    /// Tokio runtime with the time driver enabled.
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
    inner: Arc<HubInner>,
}

struct HubInner {
    enabled_kinds: Option<HashSet<String>>,
    sinks: Vec<HubSink>,
    per_sink_timeout: Duration,
    inflight: Arc<tokio::sync::Semaphore>,
    max_sink_sends_in_parallel: usize,
}

struct HubSink {
    sink: Arc<dyn Sink>,
    name: &'static str,
    state: AtomicU8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum HubSinkState {
    Ready = 0,
    NamePanicked = 1,
    Poisoned = 2,
}

impl HubSinkState {
    fn from_u8(value: u8) -> Self {
        match value {
            x if x == Self::Ready as u8 => Self::Ready,
            x if x == Self::NamePanicked as u8 => Self::NamePanicked,
            x if x == Self::Poisoned as u8 => Self::Poisoned,
            _ => unreachable!("invalid hub sink state"),
        }
    }
}

impl HubSink {
    const UNKNOWN_SINK_NAME: &str = "<unknown>";

    fn state(&self) -> HubSinkState {
        HubSinkState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn report_name_panic_once(&self) -> bool {
        self.state
            .compare_exchange(
                HubSinkState::NamePanicked as u8,
                HubSinkState::Poisoned as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn poison(&self) {
        self.state
            .store(HubSinkState::Poisoned as u8, Ordering::Release);
    }
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
            .map(|sink| {
                let (name, state) = match std::panic::catch_unwind(AssertUnwindSafe(|| sink.name()))
                {
                    Ok(name) => (name, HubSinkState::Ready),
                    Err(_) => (HubSink::UNKNOWN_SINK_NAME, HubSinkState::NamePanicked),
                };
                HubSink {
                    sink,
                    name,
                    state: AtomicU8::new(state as u8),
                }
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

    /// Explicit best-effort fire-and-forget notification.
    ///
    /// - Requires a Tokio runtime with the time driver enabled; otherwise the notification is
    ///   dropped and a warning is logged.
    /// - Concurrency is bounded; if overloaded, notifications are dropped (with a warning).
    pub fn notify_best_effort(&self, event: Event) {
        if self.inner.sinks.is_empty() {
            return;
        }
        if !self.is_kind_enabled(event.kind.as_str()) {
            return;
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn_hub_notify_dropped(event.kind.as_str(), "no_tokio_runtime");
            return;
        };
        if !has_tokio_time_driver() {
            warn_hub_notify_dropped(event.kind.as_str(), "no_tokio_time_driver");
            return;
        }

        if let Err(event) = self.try_notify_spawn(handle, event) {
            warn_hub_notify_dropped(event.kind.as_str(), "overloaded");
        }
    }

    #[deprecated(
        note = "use Hub::notify_best_effort for explicit best-effort semantics, or Hub::try_notify / Hub::send for observable delivery"
    )]
    pub fn notify(&self, event: Event) {
        self.notify_best_effort(event);
    }

    /// Attempt to enqueue a fire-and-forget notification.
    ///
    /// Returns:
    /// - `Err(TryNotifyError::NoTokioRuntime)` if called outside a Tokio runtime or without the
    ///   Tokio time driver enabled.
    /// - `Err(TryNotifyError::Overloaded)` when Hub inflight capacity is full.
    pub fn try_notify(&self, event: Event) -> Result<(), TryNotifyError> {
        if self.inner.sinks.is_empty() {
            return Ok(());
        }
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return Err(TryNotifyError::NoTokioRuntime);
        };
        if !has_tokio_time_driver() {
            return Err(TryNotifyError::NoTokioRuntime);
        }

        match self.try_notify_spawn(handle, event) {
            Ok(()) => Ok(()),
            Err(_) => Err(TryNotifyError::Overloaded),
        }
    }

    pub async fn send(&self, event: Event) -> crate::Result<()> {
        if self.inner.sinks.is_empty() {
            return Ok(());
        }
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }

        tokio::runtime::Handle::try_current()
            .map_err(|_| crate::Error::from(TryNotifyError::NoTokioRuntime))?;
        if !has_tokio_time_driver() {
            return Err(crate::Error::from(TryNotifyError::NoTokioRuntime));
        }
        let _permit = self
            .inner
            .inflight
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("hub inflight semaphore closed"))?;
        self.inner.send(&event).await
    }

    fn is_kind_enabled(&self, kind: &str) -> bool {
        let Some(enabled) = &self.inner.enabled_kinds else {
            return true;
        };
        enabled.contains(kind)
    }

    // Keep returning the original event on backpressure so callers can
    // preserve existing retry/drop behavior without reconstructing it.
    #[allow(clippy::result_large_err)]
    fn try_notify_spawn(
        &self,
        handle: tokio::runtime::Handle,
        event: Event,
    ) -> std::result::Result<(), Event> {
        let inner = self.inner.clone();

        let permit = match inner.inflight.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Err(event),
        };

        handle.spawn(async move {
            let _permit = permit;
            if let Err(err) = inner.send(&event).await {
                warn_hub_notify_failed(event.kind.as_str(), &err.to_string());
            }
        });
        Ok(())
    }
}

impl HubInner {
    async fn send_one_sink(
        timeout: Duration,
        idx: usize,
        sink: &HubSink,
        event: &Event,
    ) -> (usize, &'static str, crate::Result<()>) {
        let name = sink.name;

        match sink.state() {
            HubSinkState::Ready => {}
            HubSinkState::NamePanicked => {
                return if sink.report_name_panic_once() {
                    (idx, name, Err(anyhow::anyhow!("sink panicked").into()))
                } else {
                    (idx, name, Ok(()))
                };
            }
            HubSinkState::Poisoned => return (idx, name, Ok(())),
        }

        let result = AssertUnwindSafe(async move {
            tokio::time::timeout(timeout, sink.sink.send(event))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timeout after {timeout:?}").into()))
        })
        .catch_unwind()
        .await;
        match result {
            Ok(result) => (idx, name, result),
            Err(_) => {
                sink.poison();
                (idx, name, Err(anyhow::anyhow!("sink panicked").into()))
            }
        }
    }

    async fn send(&self, event: &Event) -> crate::Result<()> {
        if self.sinks.is_empty() {
            return Ok(());
        }
        ensure_tokio_time_driver("Hub::send")?;

        let timeout = self.per_sink_timeout;
        if self.sinks.len() == 1 {
            let (_idx, name, result) = Self::send_one_sink(timeout, 0, &self.sinks[0], event).await;
            if let Err(err) = result {
                return Err(Self::build_failures_error(vec![(0, name, err)]));
            }
            return Ok(());
        }

        let mut failures: Vec<(usize, &'static str, crate::Error)> = Vec::new();
        let max_parallel = self.max_sink_sends_in_parallel.max(1);
        let mut sink_iter = self.sinks.iter().enumerate();

        let mut pending = FuturesUnordered::new();
        for _ in 0..max_parallel {
            let Some((idx, hub_sink)) = sink_iter.next() else {
                break;
            };
            pending.push(Self::send_one_sink(timeout, idx, hub_sink, event));
        }

        while let Some((idx, name, result)) = pending.next().await {
            if let Err(err) = result {
                failures.push((idx, name, err));
            }
            if let Some((next_idx, next_hub_sink)) = sink_iter.next() {
                pending.push(Self::send_one_sink(timeout, next_idx, next_hub_sink, event));
            }
        }

        if failures.is_empty() {
            return Ok(());
        }

        Err(Self::build_failures_error(failures))
    }

    fn build_failures_error(
        mut failures: Vec<(usize, &'static str, crate::Error)>,
    ) -> crate::Error {
        if failures.len() > 1 {
            failures.sort_unstable_by_key(|(idx, _, _)| *idx);
        }
        crate::Error::from_sink_failures(
            failures
                .into_iter()
                .map(|(idx, name, err)| SinkFailure::new(idx, name, err))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::event::Severity;
    use crate::sinks::{BoxFuture, Sink};

    #[test]
    fn hub_limits_default_matches_internal_defaults() {
        let limits = HubLimits::default();
        assert_eq!(limits.max_inflight_events, DEFAULT_MAX_INFLIGHT_EVENTS);
        assert_eq!(
            limits.max_sink_sends_in_parallel,
            DEFAULT_MAX_SINK_SENDS_IN_PARALLEL
        );
    }

    #[test]
    fn hub_limits_clamp_zero_values_to_one() {
        let limits = HubLimits::default()
            .with_max_inflight_events(0)
            .with_max_sink_sends_in_parallel(0);
        assert_eq!(limits.max_inflight_events, 1);
        assert_eq!(limits.max_sink_sends_in_parallel, 1);
    }

    #[derive(Debug)]
    struct TestSink {
        name: &'static str,
        behavior: TestSinkBehavior,
        sends: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone, Copy)]
    enum TestSinkBehavior {
        Ok,
        Err,
        Sleep(Duration),
        PanicName,
        Panic,
    }

    impl Sink for TestSink {
        fn name(&self) -> &'static str {
            if matches!(self.behavior, TestSinkBehavior::PanicName) {
                panic!("name boom");
            }
            self.name
        }

        fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
            Box::pin(async move {
                self.sends.fetch_add(1, Ordering::SeqCst);
                match self.behavior {
                    TestSinkBehavior::Ok => Ok(()),
                    TestSinkBehavior::Err => Err(anyhow::anyhow!("boom").into()),
                    TestSinkBehavior::Sleep(d) => {
                        tokio::time::sleep(d).await;
                        Ok(())
                    }
                    TestSinkBehavior::PanicName => Ok(()),
                    TestSinkBehavior::Panic => panic!("boom"),
                }
            })
        }
    }

    #[test]
    fn try_notify_errors_without_tokio_runtime() {
        let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
            name: "ok",
            behavior: TestSinkBehavior::Ok,
            sends: Arc::new(AtomicUsize::new(0)),
        })];
        let hub = Hub::new(HubConfig::default(), sinks);
        let event = Event::new("kind", Severity::Info, "title");
        assert_eq!(hub.try_notify(event), Err(TryNotifyError::NoTokioRuntime));
    }

    #[test]
    fn try_notify_is_noop_without_tokio_runtime_when_no_sinks() {
        let hub = Hub::new(HubConfig::default(), Vec::new());
        let event = Event::new("kind", Severity::Info, "title");
        assert_eq!(hub.try_notify(event), Ok(()));
    }

    #[test]
    fn try_notify_is_noop_when_kind_disabled_even_without_runtime() {
        let mut enabled_kinds = BTreeSet::new();
        enabled_kinds.insert("enabled".to_string());

        let hub = Hub::new(
            HubConfig {
                enabled_kinds: Some(enabled_kinds),
                per_sink_timeout: Duration::from_secs(1),
            },
            Vec::new(),
        );

        let event = Event::new("disabled", Severity::Info, "title");
        assert_eq!(hub.try_notify(event), Ok(()));
    }

    #[test]
    fn send_is_noop_without_tokio_runtime_when_no_sinks() {
        let hub = Hub::new(HubConfig::default(), Vec::new());
        let event = Event::new("kind", Severity::Info, "title");

        let out = hub
            .send(event)
            .now_or_never()
            .expect("send should complete immediately without sinks");
        assert!(out.is_ok(), "{out:#?}");
    }

    #[test]
    fn send_aggregates_sink_failures() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![
                Arc::new(TestSink {
                    name: "ok",
                    behavior: TestSinkBehavior::Ok,
                    sends: Arc::new(AtomicUsize::new(0)),
                }),
                Arc::new(TestSink {
                    name: "bad",
                    behavior: TestSinkBehavior::Err,
                    sends: Arc::new(AtomicUsize::new(0)),
                }),
            ];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected sink failure");
            assert_eq!(err.kind(), crate::ErrorKind::SinkFailures);
            let failures = err.sink_failures().expect("structured sink failures");
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].index(), 1);
            assert_eq!(failures[0].sink_name(), "bad");
            assert_eq!(failures[0].error().kind(), crate::ErrorKind::Other);
            let msg = err.to_string();
            assert!(msg.contains("one or more sinks failed:"), "{msg}");
            assert!(msg.contains("- bad: boom"), "{msg}");
        });
    }

    #[test]
    fn send_times_out_slow_sinks() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "slow",
                behavior: TestSinkBehavior::Sleep(Duration::from_millis(50)),
                sends: Arc::new(AtomicUsize::new(0)),
            })];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_millis(5),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected timeout");
            assert_eq!(err.kind(), crate::ErrorKind::SinkFailures);
            let msg = err.to_string();
            assert!(msg.contains("timeout after"), "{msg}");
        });
    }

    #[test]
    fn try_notify_drops_when_overloaded() {
        #[derive(Debug)]
        struct CountingSink {
            counter: Arc<AtomicUsize>,
            sleep: Duration,
        }

        impl Sink for CountingSink {
            fn name(&self) -> &'static str {
                "counting"
            }

            fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
                Box::pin(async move {
                    self.counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(self.sleep).await;
                    Ok(())
                })
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let counter = Arc::new(AtomicUsize::new(0));
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(CountingSink {
                counter: counter.clone(),
                sleep: Duration::from_millis(50),
            })];

            let hub = Hub::new_with_inflight_limit(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
                1,
            );

            hub.try_notify(Event::new("kind", Severity::Info, "t1"))
                .expect("first notify ok");
            assert_eq!(
                hub.try_notify(Event::new("kind", Severity::Info, "t2")),
                Err(TryNotifyError::Overloaded)
            );

            tokio::time::sleep(Duration::from_millis(80)).await;
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn send_includes_sink_name_on_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "panic",
                behavior: TestSinkBehavior::Panic,
                sends: Arc::new(AtomicUsize::new(0)),
            })];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected panic failure");
            assert_eq!(err.kind(), crate::ErrorKind::SinkFailures);
            let msg = err.to_string();
            assert!(msg.contains("- panic:"), "{msg}");
        });
    }

    #[test]
    fn send_handles_sink_name_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "ignored",
                behavior: TestSinkBehavior::PanicName,
                sends: Arc::new(AtomicUsize::new(0)),
            })];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected panic failure");
            assert_eq!(err.kind(), crate::ErrorKind::SinkFailures);
            let msg = err.to_string();
            assert!(msg.contains("- <unknown>: sink panicked"), "{msg}");
        });
    }

    #[test]
    fn send_reports_failures_in_sink_order() {
        #[derive(Debug)]
        struct DelayedFailSink {
            name: &'static str,
            sleep: Duration,
        }

        impl Sink for DelayedFailSink {
            fn name(&self) -> &'static str {
                self.name
            }

            fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
                Box::pin(async move {
                    tokio::time::sleep(self.sleep).await;
                    Err(anyhow::anyhow!("boom").into())
                })
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![
                Arc::new(DelayedFailSink {
                    name: "first",
                    sleep: Duration::from_millis(40),
                }),
                Arc::new(DelayedFailSink {
                    name: "second",
                    sleep: Duration::from_millis(1),
                }),
            ];
            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected sink failure");
            let failures = err.sink_failures().expect("structured sink failures");
            assert_eq!(failures.len(), 2);
            assert_eq!(failures[0].sink_name(), "first");
            assert_eq!(failures[1].sink_name(), "second");
            let msg = err.to_string();
            let first = msg.find("- first:").expect("contains first");
            let second = msg.find("- second:").expect("contains second");
            assert!(first < second, "{msg}");
        });
    }

    #[test]
    fn send_keeps_aggregate_failure_sources_inspectable() {
        #[derive(Debug)]
        struct IoFailSink;

        impl Sink for IoFailSink {
            fn name(&self) -> &'static str {
                "io-fail"
            }

            fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
                Box::pin(async move { Err(std::io::Error::other("dial failed").into()) })
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(IoFailSink)];
            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );

            let err = hub
                .send(Event::new("kind", Severity::Info, "title"))
                .await
                .expect_err("expected sink failure");

            let aggregate_source = err.source().expect("aggregate source");
            assert_eq!(aggregate_source.to_string(), "- io-fail: dial failed");

            let failure = err.sink_failures().expect("structured failures")[0]
                .source()
                .expect("sink failure source");
            assert_eq!(failure.to_string(), "dial failed");
            assert!(
                failure.source().is_none(),
                "root error should stay unchanged"
            );
        });
    }

    #[test]
    fn send_respects_sink_parallel_limit() {
        #[derive(Debug)]
        struct TrackingSink {
            current: Arc<AtomicUsize>,
            max_seen: Arc<AtomicUsize>,
            sleep: Duration,
        }

        impl Sink for TrackingSink {
            fn name(&self) -> &'static str {
                "tracking"
            }

            fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
                Box::pin(async move {
                    let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                    self.max_seen.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(self.sleep).await;
                    self.current.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let current = Arc::new(AtomicUsize::new(0));
            let max_seen = Arc::new(AtomicUsize::new(0));
            let sinks: Vec<Arc<dyn Sink>> = (0..3)
                .map(|_| {
                    Arc::new(TrackingSink {
                        current: current.clone(),
                        max_seen: max_seen.clone(),
                        sleep: Duration::from_millis(20),
                    }) as Arc<dyn Sink>
                })
                .collect();

            let hub = Hub::new_with_limits(
                HubConfig::default(),
                sinks,
                HubLimits::default()
                    .with_max_inflight_events(8)
                    .with_max_sink_sends_in_parallel(1),
            );

            hub.send(Event::new("kind", Severity::Info, "title"))
                .await
                .expect("send ok");
            assert_eq!(max_seen.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn send_returns_error_without_tokio_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "ok",
                behavior: TestSinkBehavior::Ok,
                sends: Arc::new(AtomicUsize::new(0)),
            })];
            let hub = Hub::new(HubConfig::default(), sinks);

            let err = hub
                .send(Event::new("kind", Severity::Info, "title"))
                .await
                .expect_err("missing time driver should fail");
            assert_eq!(err.kind(), crate::ErrorKind::RuntimeUnavailable);
            let msg = err.to_string();
            assert!(msg.contains("time driver"), "{msg}");
        });
    }

    #[test]
    fn send_poisons_panicking_sink_after_first_failure() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let panic_sends = Arc::new(AtomicUsize::new(0));
            let healthy_sends = Arc::new(AtomicUsize::new(0));
            let sinks: Vec<Arc<dyn Sink>> = vec![
                Arc::new(TestSink {
                    name: "panic",
                    behavior: TestSinkBehavior::Panic,
                    sends: panic_sends.clone(),
                }),
                Arc::new(TestSink {
                    name: "healthy",
                    behavior: TestSinkBehavior::Ok,
                    sends: healthy_sends.clone(),
                }),
            ];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );

            let first_err = hub
                .send(Event::new("kind", Severity::Info, "first"))
                .await
                .expect_err("first send should report panic sink");
            let failures = first_err.sink_failures().expect("structured sink failures");
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].sink_name(), "panic");

            hub.send(Event::new("kind", Severity::Info, "second"))
                .await
                .expect("poisoned sink should be skipped");

            assert_eq!(panic_sends.load(Ordering::SeqCst), 1);
            assert_eq!(healthy_sends.load(Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn send_poisons_sink_whose_name_panics() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sends = Arc::new(AtomicUsize::new(0));
            let sinks: Vec<Arc<dyn Sink>> = vec![
                Arc::new(TestSink {
                    name: "ignored",
                    behavior: TestSinkBehavior::PanicName,
                    sends: sends.clone(),
                }),
                Arc::new(TestSink {
                    name: "healthy",
                    behavior: TestSinkBehavior::Ok,
                    sends: Arc::new(AtomicUsize::new(0)),
                }),
            ];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );

            let first_err = hub
                .send(Event::new("kind", Severity::Info, "first"))
                .await
                .expect_err("first send should report name panic");
            let failures = first_err.sink_failures().expect("structured sink failures");
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].sink_name(), "<unknown>");

            hub.send(Event::new("kind", Severity::Info, "second"))
                .await
                .expect("poisoned sink should be skipped");

            assert_eq!(sends.load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn try_notify_errors_without_tokio_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "ok",
                behavior: TestSinkBehavior::Ok,
                sends: Arc::new(AtomicUsize::new(0)),
            })];
            let hub = Hub::new(HubConfig::default(), sinks);

            assert_eq!(
                hub.try_notify(Event::new("kind", Severity::Info, "title")),
                Err(TryNotifyError::NoTokioRuntime)
            );
        });
    }
}
