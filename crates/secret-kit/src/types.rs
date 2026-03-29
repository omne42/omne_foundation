use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};

use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};
use structured_text_kit::{CatalogTextRef, StructuredText, structured_text};

#[derive(Debug)]
pub enum SecretError {
    Io {
        text: StructuredText,
        source: std::io::Error,
    },
    Json {
        text: StructuredText,
        source: serde_json::Error,
    },
    Lookup(StructuredText),
    InvalidSpec(StructuredText),
    Command(StructuredText),
}

pub type Result<T> = std::result::Result<T, SecretError>;

impl SecretError {
    fn io_retry_advice(source: &std::io::Error) -> ErrorRetryAdvice {
        match source.kind() {
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::Unsupported => ErrorRetryAdvice::DoNotRetry,
            _ => ErrorRetryAdvice::Retryable,
        }
    }

    fn command_retry_advice(text: &StructuredText) -> ErrorRetryAdvice {
        match text.as_catalog().map(CatalogTextRef::code) {
            Some("error_detail.secret.command_timeout")
            | Some("error_detail.secret.command_spawn_failed")
            | Some("error_detail.secret.command_output_read_failed") => ErrorRetryAdvice::Retryable,
            _ => ErrorRetryAdvice::DoNotRetry,
        }
    }

    #[must_use]
    pub fn structured_text(&self) -> &StructuredText {
        match self {
            Self::Io { text, .. }
            | Self::Json { text, .. }
            | Self::Lookup(text)
            | Self::InvalidSpec(text)
            | Self::Command(text) => text,
        }
    }

    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::Io { .. } => {
                ErrorCode::try_new("secret.io").expect("literal error code should validate")
            }
            Self::Json { .. } => {
                ErrorCode::try_new("secret.json").expect("literal error code should validate")
            }
            Self::Lookup(_) => {
                ErrorCode::try_new("secret.lookup").expect("literal error code should validate")
            }
            Self::InvalidSpec(_) => ErrorCode::try_new("secret.invalid_spec")
                .expect("literal error code should validate"),
            Self::Command(_) => {
                ErrorCode::try_new("secret.command").expect("literal error code should validate")
            }
        }
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        match self {
            Self::Io { .. } => ErrorCategory::ExternalDependency,
            Self::Json { .. } => ErrorCategory::InvalidInput,
            Self::Lookup(_) => ErrorCategory::NotFound,
            Self::InvalidSpec(_) => ErrorCategory::InvalidInput,
            Self::Command(_) => ErrorCategory::ExternalDependency,
        }
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        match self {
            Self::Io { source, .. } => Self::io_retry_advice(source),
            Self::Json { .. } => ErrorRetryAdvice::DoNotRetry,
            Self::Lookup(_) => ErrorRetryAdvice::DoNotRetry,
            Self::InvalidSpec(_) => ErrorRetryAdvice::DoNotRetry,
            Self::Command(text) => Self::command_retry_advice(text),
        }
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        ErrorRecord::new(self.error_code(), self.structured_text().clone())
            .with_category(self.error_category())
            .with_retry_advice(self.retry_advice())
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        let category = self.error_category();
        let retry_advice = self.retry_advice();
        match self {
            Self::Io { text, source } => ErrorRecord::new(
                ErrorCode::try_new("secret.io").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice)
            .with_source(source),
            Self::Json { text, source } => ErrorRecord::new(
                ErrorCode::try_new("secret.json").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice)
            .with_source(source),
            Self::Lookup(text) => ErrorRecord::new(
                ErrorCode::try_new("secret.lookup").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice),
            Self::InvalidSpec(text) => ErrorRecord::new(
                ErrorCode::try_new("secret.invalid_spec")
                    .expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice),
            Self::Command(text) => ErrorRecord::new(
                ErrorCode::try_new("secret.command").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice),
        }
    }

    pub(crate) fn io(text: StructuredText, source: std::io::Error) -> Self {
        Self::Io { text, source }
    }

    pub(crate) fn json(text: StructuredText, source: serde_json::Error) -> Self {
        Self::Json { text, source }
    }

    pub(crate) fn lookup(text: StructuredText) -> Self {
        Self::Lookup(text)
    }
}

impl Display for SecretError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { text, source } => write!(
                f,
                "secret io error: {}: {source}",
                text.diagnostic_display()
            ),
            Self::Json { text, source } => {
                write!(
                    f,
                    "secret json error: {}: {source}",
                    text.diagnostic_display()
                )
            }
            Self::Lookup(text) => {
                write!(f, "secret lookup error: {}", text.diagnostic_display())
            }
            Self::InvalidSpec(text) => {
                write!(f, "invalid secret spec: {}", text.diagnostic_display())
            }
            Self::Command(text) => {
                write!(f, "secret command error: {}", text.diagnostic_display())
            }
        }
    }
}

impl StdError for SecretError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Lookup(_) | Self::InvalidSpec(_) | Self::Command(_) => None,
        }
    }
}

impl From<std::io::Error> for SecretError {
    fn from(source: std::io::Error) -> Self {
        let error = source.to_string();
        Self::io(
            structured_text!("error_detail.secret.io_error", "error" => error),
            source,
        )
    }
}

impl From<serde_json::Error> for SecretError {
    fn from(source: serde_json::Error) -> Self {
        let error = source.to_string();
        Self::json(
            structured_text!("error_detail.secret.json_error", "error" => error),
            source,
        )
    }
}

impl From<SecretError> for ErrorRecord {
    fn from(error: SecretError) -> Self {
        error.into_error_record()
    }
}
