//! Provider-agnostic notification primitives.
//!
//! This module is the narrow foundation surface for downstream code that only needs the event,
//! hub, error, and sink abstractions. Built-in transport implementations live under
//! [`crate::providers`], while env wiring stays in [`crate::env`] when that feature is enabled.
//!
//! ```rust
//! use notify_kit::core::{Event, HubConfig, Result, Severity};
//! use notify_kit::{Event as RootEvent, HubConfig as RootHubConfig, Severity as RootSeverity};
//!
//! let _ = HubConfig::default();
//! let _ = RootHubConfig::default();
//! let _ = Event::new("kind", Severity::Info, "title");
//! let _ = RootEvent::new("kind", RootSeverity::Info, "title");
//! let _: Result<()> = Ok(());
//! ```

pub use crate::error::{Error, ErrorKind, SinkFailure};
pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, HubLimits, TryNotifyError};
pub use crate::sinks::Sink;

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_module_exposes_foundation_types() {
        let _ = HubConfig::default();
        let _ = HubLimits::default();
        let _ = Event::new("kind", Severity::Info, "title");
        let _: Result<()> = Ok(());
    }
}
