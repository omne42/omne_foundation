use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};

use structured_text_kit::StructuredText;

use crate::ErrorCode;
#[cfg(feature = "cli")]
use crate::{CliError, CliExitCode};

type BoxError = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCategory {
    InvalidInput,
    NotFound,
    Conflict,
    PermissionDenied,
    Unauthenticated,
    RateLimited,
    Timeout,
    Unavailable,
    ExternalDependency,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorRetryAdvice {
    DoNotRetry,
    Retryable,
}

#[derive(Debug)]
pub struct ErrorRecord {
    code: ErrorCode,
    category: ErrorCategory,
    retry_advice: ErrorRetryAdvice,
    user_text: StructuredText,
    diagnostic_text: Option<StructuredText>,
    source: Option<BoxError>,
}

impl ErrorRecord {
    #[must_use]
    pub fn new(code: ErrorCode, user_text: StructuredText) -> Self {
        Self {
            code,
            category: ErrorCategory::Internal,
            retry_advice: ErrorRetryAdvice::DoNotRetry,
            user_text,
            diagnostic_text: None,
            source: None,
        }
    }

    #[must_use]
    pub fn new_freeform(code: ErrorCode, user_text: impl Into<String>) -> Self {
        Self::new(code, StructuredText::freeform(user_text))
    }

    #[must_use]
    pub fn with_category(mut self, category: ErrorCategory) -> Self {
        self.category = category;
        self
    }

    #[must_use]
    pub fn with_retry_advice(mut self, retry_advice: ErrorRetryAdvice) -> Self {
        self.retry_advice = retry_advice;
        self
    }

    #[must_use]
    pub fn with_diagnostic_text(mut self, diagnostic_text: StructuredText) -> Self {
        self.diagnostic_text = Some(diagnostic_text);
        self
    }

    #[must_use]
    pub fn with_freeform_diagnostic_text(mut self, diagnostic_text: impl Into<String>) -> Self {
        self.diagnostic_text = Some(StructuredText::freeform(diagnostic_text));
        self
    }

    #[must_use]
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }

    #[cfg(feature = "cli")]
    #[must_use]
    pub fn with_exit_code<E>(self, exit_code: E) -> CliError<E>
    where
        E: CliExitCode,
    {
        CliError::new(self, exit_code)
    }

    #[must_use]
    pub fn code(&self) -> &ErrorCode {
        &self.code
    }

    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        self.category
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        self.retry_advice
    }

    #[must_use]
    pub fn user_text(&self) -> &StructuredText {
        &self.user_text
    }

    #[must_use]
    pub fn diagnostic_text(&self) -> Option<&StructuredText> {
        self.diagnostic_text.as_ref()
    }

    #[must_use]
    pub fn display_text(&self) -> &StructuredText {
        self.diagnostic_text.as_ref().unwrap_or(&self.user_text)
    }

    #[must_use]
    pub fn source_ref(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        self.source.as_deref()
    }
}

impl Display for ErrorRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            self.code,
            self.display_text().diagnostic_display()
        )
    }
}

impl StdError for ErrorRecord {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}
