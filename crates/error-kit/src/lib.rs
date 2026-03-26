#![forbid(unsafe_code)]

//! Structured error primitives built on top of `structured-text-kit`.
//!
//! This crate is intentionally about error-domain semantics, not generic text primitives and not
//! transport DTOs. The user-visible or operator-visible text carried by an error is represented as
//! [`structured_text_kit::StructuredText`], while error metadata such as stable codes, categories,
//! retry advice, and causal sources live here.

#[cfg(feature = "cli")]
mod cli;
mod code;
mod record;

#[cfg(feature = "cli")]
pub use cli::{CliError, CliExitCode, CliResult};
pub use code::{ErrorCode, ErrorCodeValidationError};
pub use record::{ErrorCategory, ErrorRecord, ErrorRetryAdvice};

#[cfg(test)]
mod tests;
