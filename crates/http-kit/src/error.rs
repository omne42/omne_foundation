use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::Message(_) => literal_error_code("http_kit.message"),
            Self::Other(err) => classify_anyhow(err).0,
        }
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        match self {
            Self::Message(_) => ErrorCategory::InvalidInput,
            Self::Other(err) => classify_anyhow(err).1,
        }
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        match self {
            Self::Message(_) => ErrorRetryAdvice::DoNotRetry,
            Self::Other(err) => classify_anyhow(err).2,
        }
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        match self {
            Self::Message(message) => ErrorRecord::new_freeform(
                literal_error_code("http_kit.message"),
                "http-kit rejected invalid input",
            )
            .with_category(ErrorCategory::InvalidInput)
            .with_retry_advice(ErrorRetryAdvice::DoNotRetry)
            .with_freeform_diagnostic_text(message.clone()),
            Self::Other(err) => build_anyhow_error_record(err),
        }
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        match self {
            Self::Message(message) => ErrorRecord::new_freeform(
                literal_error_code("http_kit.message"),
                "http-kit rejected invalid input",
            )
            .with_category(ErrorCategory::InvalidInput)
            .with_retry_advice(ErrorRetryAdvice::DoNotRetry)
            .with_freeform_diagnostic_text(message),
            Self::Other(err) => build_anyhow_error_record(&err),
        }
    }
}

impl From<Error> for ErrorRecord {
    fn from(error: Error) -> Self {
        error.into_error_record()
    }
}

fn literal_error_code(code: &'static str) -> ErrorCode {
    ErrorCode::try_new(code).expect("literal error code should validate")
}

fn classify_anyhow(err: &anyhow::Error) -> (ErrorCode, ErrorCategory, ErrorRetryAdvice) {
    if let Some(reqwest) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
    {
        if reqwest.is_timeout() {
            return (
                literal_error_code("http_kit.reqwest.timeout"),
                ErrorCategory::Timeout,
                ErrorRetryAdvice::Retryable,
            );
        }
        if reqwest.is_connect() {
            return (
                literal_error_code("http_kit.reqwest.connect"),
                ErrorCategory::Unavailable,
                ErrorRetryAdvice::Retryable,
            );
        }
        if reqwest.is_decode() {
            return (
                literal_error_code("http_kit.reqwest.decode"),
                ErrorCategory::ExternalDependency,
                ErrorRetryAdvice::DoNotRetry,
            );
        }
        return (
            literal_error_code("http_kit.reqwest"),
            ErrorCategory::ExternalDependency,
            ErrorRetryAdvice::Retryable,
        );
    }

    if let Some(io) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
    {
        return match io.kind() {
            std::io::ErrorKind::NotFound => (
                literal_error_code("http_kit.io.not_found"),
                ErrorCategory::NotFound,
                ErrorRetryAdvice::DoNotRetry,
            ),
            std::io::ErrorKind::TimedOut => (
                literal_error_code("http_kit.io.timeout"),
                ErrorCategory::Timeout,
                ErrorRetryAdvice::Retryable,
            ),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => (
                literal_error_code("http_kit.io.invalid_input"),
                ErrorCategory::InvalidInput,
                ErrorRetryAdvice::DoNotRetry,
            ),
            _ => (
                literal_error_code("http_kit.io"),
                ErrorCategory::ExternalDependency,
                ErrorRetryAdvice::Retryable,
            ),
        };
    }

    if err
        .chain()
        .any(|cause| cause.downcast_ref::<serde_json::Error>().is_some())
    {
        return (
            literal_error_code("http_kit.json"),
            ErrorCategory::ExternalDependency,
            ErrorRetryAdvice::DoNotRetry,
        );
    }

    (
        literal_error_code("http_kit.other"),
        ErrorCategory::ExternalDependency,
        ErrorRetryAdvice::DoNotRetry,
    )
}

fn build_anyhow_error_record(err: &anyhow::Error) -> ErrorRecord {
    let (code, category, retry_advice) = classify_anyhow(err);
    ErrorRecord::new_freeform(code, "http-kit request failed")
        .with_category(category)
        .with_retry_advice(retry_advice)
        .with_freeform_diagnostic_text(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::Error;
    use error_kit::{ErrorCategory, ErrorRetryAdvice};

    #[test]
    fn io_timeouts_map_to_retryable_timeout_records() {
        let err = Error::from(anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timeout",
        )));
        let record = err.error_record();

        assert_eq!(record.code().as_str(), "http_kit.io.timeout");
        assert_eq!(record.category(), ErrorCategory::Timeout);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::Retryable);
    }

    #[test]
    fn message_errors_map_to_invalid_input_records() {
        let record = Error::Message("bad header".to_string()).into_error_record();

        assert_eq!(record.code().as_str(), "http_kit.message");
        assert_eq!(record.category(), ErrorCategory::InvalidInput);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        let diagnostic = format!(
            "{}",
            record
                .diagnostic_text()
                .expect("diagnostic text")
                .diagnostic_display()
        );
        assert!(diagnostic.contains("bad header"));
    }
}
