use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::sync::OnceLock;

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
    Provider(StructuredText),
}

pub type Result<T> = std::result::Result<T, SecretError>;

static SECRET_IO_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret.io");
static SECRET_JSON_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret.json");
static SECRET_LOOKUP_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret.lookup");
static SECRET_INVALID_SPEC_ERROR_CODE: SecretErrorCode =
    SecretErrorCode::new("secret.invalid_spec");
static SECRET_COMMAND_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret.command");
static SECRET_PROVIDER_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret.provider");
static SECRET_INTERNAL_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret.internal");

struct SecretErrorCode {
    literal: &'static str,
    parsed: OnceLock<ErrorCode>,
}

impl SecretErrorCode {
    const fn new(literal: &'static str) -> Self {
        Self {
            literal,
            parsed: OnceLock::new(),
        }
    }

    fn get(&'static self) -> ErrorCode {
        self.parsed
            .get_or_init(|| parse_error_code_literal(self.literal))
            .clone()
    }
}

#[derive(Clone, Copy)]
struct SecretErrorMapping {
    code: &'static SecretErrorCode,
    category: ErrorCategory,
}

impl SecretErrorMapping {
    const fn new(code: &'static SecretErrorCode, category: ErrorCategory) -> Self {
        Self { code, category }
    }

    fn new_record(self, text: StructuredText, retry_advice: ErrorRetryAdvice) -> ErrorRecord {
        ErrorRecord::new(self.code.get(), text)
            .with_category(self.category)
            .with_retry_advice(retry_advice)
    }
}

fn parse_error_code_literal(literal: &'static str) -> ErrorCode {
    ErrorCode::try_new(literal).unwrap_or_else(|_| SECRET_INTERNAL_ERROR_CODE.get())
}

impl SecretError {
    fn mapping(&self) -> SecretErrorMapping {
        match self {
            Self::Io { .. } => Self::io_mapping(),
            Self::Json { .. } => Self::json_mapping(),
            Self::Lookup(_) => Self::lookup_mapping(),
            Self::InvalidSpec(_) => Self::invalid_spec_mapping(),
            Self::Command(_) => Self::command_mapping(),
            Self::Provider(_) => Self::provider_mapping(),
        }
    }

    const fn io_mapping() -> SecretErrorMapping {
        SecretErrorMapping::new(&SECRET_IO_ERROR_CODE, ErrorCategory::ExternalDependency)
    }

    const fn json_mapping() -> SecretErrorMapping {
        SecretErrorMapping::new(&SECRET_JSON_ERROR_CODE, ErrorCategory::InvalidInput)
    }

    const fn lookup_mapping() -> SecretErrorMapping {
        SecretErrorMapping::new(&SECRET_LOOKUP_ERROR_CODE, ErrorCategory::NotFound)
    }

    const fn invalid_spec_mapping() -> SecretErrorMapping {
        SecretErrorMapping::new(&SECRET_INVALID_SPEC_ERROR_CODE, ErrorCategory::InvalidInput)
    }

    const fn command_mapping() -> SecretErrorMapping {
        SecretErrorMapping::new(
            &SECRET_COMMAND_ERROR_CODE,
            ErrorCategory::ExternalDependency,
        )
    }

    const fn provider_mapping() -> SecretErrorMapping {
        SecretErrorMapping::new(
            &SECRET_PROVIDER_ERROR_CODE,
            ErrorCategory::ExternalDependency,
        )
    }

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

    fn provider_retry_advice(text: &StructuredText) -> ErrorRetryAdvice {
        match text.as_catalog().map(CatalogTextRef::code) {
            Some("error_detail.secret.keyring_no_storage_access")
            | Some("error_detail.secret.keyring_platform_failure")
            | Some("error_detail.secret.keyring_blocking_task_failed") => {
                ErrorRetryAdvice::Retryable
            }
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
            | Self::Command(text)
            | Self::Provider(text) => text,
        }
    }

    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        self.mapping().code.get()
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        self.mapping().category
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        match self {
            Self::Io { source, .. } => Self::io_retry_advice(source),
            Self::Json { .. } => ErrorRetryAdvice::DoNotRetry,
            Self::Lookup(_) => ErrorRetryAdvice::DoNotRetry,
            Self::InvalidSpec(_) => ErrorRetryAdvice::DoNotRetry,
            Self::Command(text) => Self::command_retry_advice(text),
            Self::Provider(text) => Self::provider_retry_advice(text),
        }
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        self.mapping()
            .new_record(self.structured_text().clone(), self.retry_advice())
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        let retry_advice = self.retry_advice();
        match self {
            Self::Io { text, source } => Self::io_mapping()
                .new_record(text, retry_advice)
                .with_source(source),
            Self::Json { text, source } => Self::json_mapping()
                .new_record(text, retry_advice)
                .with_source(source),
            Self::Lookup(text) => Self::lookup_mapping().new_record(text, retry_advice),
            Self::InvalidSpec(text) => Self::invalid_spec_mapping().new_record(text, retry_advice),
            Self::Command(text) => Self::command_mapping().new_record(text, retry_advice),
            Self::Provider(text) => Self::provider_mapping().new_record(text, retry_advice),
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
            Self::Provider(text) => {
                write!(f, "secret provider error: {}", text.diagnostic_display())
            }
        }
    }
}

impl StdError for SecretError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Lookup(_) | Self::InvalidSpec(_) | Self::Command(_) | Self::Provider(_) => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_literal_error_code_falls_back_to_internal_code() {
        static INVALID_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret invalid spec");

        assert_eq!(INVALID_ERROR_CODE.get().as_str(), "secret.internal");
    }

    #[test]
    fn invalid_literal_error_record_uses_internal_code_without_losing_metadata() {
        static INVALID_ERROR_CODE: SecretErrorCode = SecretErrorCode::new("secret invalid spec");

        let record = SecretErrorMapping::new(&INVALID_ERROR_CODE, ErrorCategory::InvalidInput)
            .new_record(
                structured_text!("error_detail.secret.not_resolvable"),
                ErrorRetryAdvice::DoNotRetry,
            );

        assert_eq!(record.code().as_str(), "secret.internal");
        assert_eq!(record.category(), ErrorCategory::InvalidInput);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    }

    #[test]
    fn secret_error_keeps_existing_code_for_valid_literals() {
        let error = SecretError::Lookup(structured_text!("error_detail.secret.not_resolvable"));

        assert_eq!(error.error_code().as_str(), "secret.lookup");
        assert_eq!(error.error_record().code().as_str(), "secret.lookup");
        assert_eq!(error.into_error_record().code().as_str(), "secret.lookup");
    }
}
