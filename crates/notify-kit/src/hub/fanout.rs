use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::stream::{FuturesUnordered, StreamExt};

use crate::error::SinkFailure;
use crate::event::Event;

use super::{HubInner, HubSink};

impl HubInner {
    async fn send_one_sink(
        timeout: Duration,
        idx: usize,
        sink: &HubSink,
        event: &Event,
    ) -> (usize, &'static str, crate::Result<()>) {
        const UNKNOWN_SINK_NAME: &str = "<unknown>";

        let Some(name) = sink.name else {
            return (
                idx,
                UNKNOWN_SINK_NAME,
                Err(anyhow::anyhow!("sink panicked").into()),
            );
        };
        let result = AssertUnwindSafe(async move {
            tokio::time::timeout(timeout, sink.sink.send(event))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timeout after {timeout:?}").into()))
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("sink panicked").into()));
        (idx, name, result)
    }

    pub(super) async fn send(&self, event: &Event) -> crate::Result<()> {
        if self.sinks.is_empty() {
            return Ok(());
        }
        super::runtime::ensure_tokio_time_driver("Hub::send")?;

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
