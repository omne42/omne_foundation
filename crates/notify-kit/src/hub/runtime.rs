use std::time::Duration;

use crate::Event;
use crate::log::warn_hub_notify_dropped;

use super::{Hub, TryNotifyError};

pub(super) fn ensure_tokio_time_driver(operation: &'static str) -> crate::Result<()> {
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

impl Hub {
    /// Fire-and-forget notification.
    ///
    /// - Requires a Tokio runtime with the time driver enabled; otherwise the notification is
    ///   dropped and a warning is logged.
    /// - Concurrency is bounded; if overloaded, notifications are dropped (with a warning).
    pub fn notify(&self, mut event: Event) {
        if self.inner.sinks.is_empty() {
            return;
        }
        if !self.is_kind_enabled(event.kind.as_str()) {
            return;
        }
        event.normalize_delivery_views();

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

    /// Attempt to enqueue a fire-and-forget notification.
    ///
    /// Returns:
    /// - `Err(TryNotifyError::NoTokioRuntime)` if called outside a Tokio runtime or without the
    ///   Tokio time driver enabled.
    /// - `Err(TryNotifyError::Overloaded)` when Hub inflight capacity is full.
    pub fn try_notify(&self, mut event: Event) -> Result<(), TryNotifyError> {
        if self.inner.sinks.is_empty() {
            return Ok(());
        }
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }
        event.normalize_delivery_views();

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

    pub async fn send(&self, mut event: Event) -> crate::Result<()> {
        if self.inner.sinks.is_empty() {
            return Ok(());
        }
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }
        event.normalize_delivery_views();

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
}
