use crate::Event;
use crate::log::warn_hub_notify_failed;

use super::Hub;

impl Hub {
    // Keep returning the original event on backpressure so callers can
    // preserve existing retry/drop behavior without reconstructing it.
    #[allow(clippy::result_large_err)]
    pub(super) fn try_notify_spawn(
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
