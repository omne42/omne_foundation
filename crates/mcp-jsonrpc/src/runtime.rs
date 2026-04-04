use std::time::Duration;

use crate::{Error, ProtocolErrorKind};

const TOKIO_TIME_DRIVER_ERROR: &str =
    "tokio runtime time driver is not enabled; build the runtime with enable_time()";

pub(crate) fn ensure_tokio_time_driver(operation: &'static str) -> Result<(), Error> {
    std::panic::catch_unwind(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    })
    .map_err(|_| {
        Error::protocol(
            ProtocolErrorKind::Other,
            format!("{TOKIO_TIME_DRIVER_ERROR} ({operation})"),
        )
    })
}
