#![forbid(unsafe_code)]

//! Structured log records built on top of `tracing`.
//!
//! This crate does not replace the `tracing` ecosystem. Instead, it provides a small, explicit
//! domain model for log records that need stable machine codes, optional structured user text, and
//! a bounded set of machine-readable fields before being emitted into `tracing`.

mod code;
mod field;
mod record;

pub use code::{LogCode, LogCodeValidationError};
pub use field::{LogFieldNameValidationError, LogValue};
pub use record::{LogLevel, LogRecord};

#[cfg(test)]
mod tests;
