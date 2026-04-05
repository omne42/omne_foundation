use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::FutureExt;
use structured_text_kit::structured_text;

use super::*;
use crate::event::Event;
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
            }),
            Arc::new(TestSink {
                name: "bad",
                behavior: TestSinkBehavior::Err,
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
fn send_normalizes_event_views_before_sink_fanout() {
    #[derive(Debug)]
    struct RecordingSink {
        title: Arc<std::sync::Mutex<Vec<String>>>,
        body: Arc<std::sync::Mutex<Vec<Option<String>>>>,
        tag: Arc<std::sync::Mutex<Vec<Option<String>>>>,
    }

    impl Sink for RecordingSink {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
            Box::pin(async move {
                self.title
                    .lock()
                    .expect("title lock")
                    .push(event.title.clone());
                self.body
                    .lock()
                    .expect("body lock")
                    .push(event.body.clone());
                self.tag
                    .lock()
                    .expect("tag lock")
                    .push(event.tags.get("thread_id").cloned());
                Ok(())
            })
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        let title = Arc::new(std::sync::Mutex::new(Vec::new()));
        let body = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tag = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink: Arc<dyn Sink> = Arc::new(RecordingSink {
            title: Arc::clone(&title),
            body: Arc::clone(&body),
            tag: Arc::clone(&tag),
        });

        let hub = Hub::new(HubConfig::default(), vec![sink]);
        let mut event = Event::new("kind", Severity::Info, "plain");
        event.title = "stale-title".to_string();
        event.body = Some("stale-body".to_string());
        event
            .tags
            .insert("thread_id".to_string(), "stale".to_string());
        event = event
            .with_title_text(structured_text!("notify.title", "repo" => "omne"))
            .with_body_text(structured_text!("notify.body", "step" => "review"))
            .with_tag_text(
                "thread_id",
                structured_text!("notify.tag", "value" => "fresh"),
            );

        hub.send(event).await.expect("hub send");

        assert_eq!(
            title.lock().expect("title values").as_slice(),
            &[structured_text!("notify.title", "repo" => "omne").to_string()]
        );
        assert_eq!(
            body.lock().expect("body values").as_slice(),
            &[Some(
                structured_text!("notify.body", "step" => "review").to_string()
            )]
        );
        assert_eq!(
            tag.lock().expect("tag values").as_slice(),
            &[Some(
                structured_text!("notify.tag", "value" => "fresh").to_string()
            )]
        );
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
fn try_notify_errors_without_tokio_time_driver() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
            name: "ok",
            behavior: TestSinkBehavior::Ok,
        })];
        let hub = Hub::new(HubConfig::default(), sinks);

        assert_eq!(
            hub.try_notify(Event::new("kind", Severity::Info, "title")),
            Err(TryNotifyError::NoTokioRuntime)
        );
    });
}
