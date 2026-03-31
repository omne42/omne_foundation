//! Core notification boundary.
//!
//! This namespace intentionally contains the reusable notification contract only:
//! event data, sink trait, hub runtime, and error types. Built-in provider sinks
//! remain available under [`crate::builtin`].

pub use crate::error::{Error, ErrorKind, SinkFailure};
pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, HubLimits, TryNotifyError};
pub use crate::secret::NotifySecret;
pub use crate::sinks::Sink;
pub type Result<T> = std::result::Result<T, Error>;
