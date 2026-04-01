use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Other,
    RuntimeUnavailable,
    SinkFailures,
}

#[derive(Debug)]
pub struct SinkFailure {
    index: usize,
    sink_name: &'static str,
    error: Box<Error>,
}

impl SinkFailure {
    pub fn new(index: usize, sink_name: &'static str, error: Error) -> Self {
        Self {
            index,
            sink_name,
            error: Box::new(error),
        }
    }

    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    #[must_use]
    pub fn sink_name(&self) -> &'static str {
        self.sink_name
    }

    #[must_use]
    pub fn error(&self) -> &Error {
        &self.error
    }
}

impl std::fmt::Display for SinkFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "- {}: {:#}", self.sink_name, self.error)
        } else {
            write!(f, "- {}: {}", self.sink_name, self.error)
        }
    }
}

impl std::error::Error for SinkFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

#[derive(Debug)]
enum ErrorRepr {
    Other(anyhow::Error),
    SinkFailures(Vec<SinkFailure>),
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    repr: ErrorRepr,
}

impl Error {
    fn new(kind: ErrorKind, repr: ErrorRepr) -> Self {
        Self { kind, repr }
    }

    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn sink_failures(&self) -> Option<&[SinkFailure]> {
        match &self.repr {
            ErrorRepr::SinkFailures(failures) => Some(failures.as_slice()),
            ErrorRepr::Other(_) => None,
        }
    }

    #[must_use]
    pub fn from_sink_failures(failures: Vec<SinkFailure>) -> Self {
        Self::new(ErrorKind::SinkFailures, ErrorRepr::SinkFailures(failures))
    }

    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        literal_error_code(match self.kind {
            ErrorKind::Other => "notify_kit.other",
            ErrorKind::RuntimeUnavailable => "notify_kit.runtime_unavailable",
            ErrorKind::SinkFailures => "notify_kit.sink_failures",
        })
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        match self.kind {
            ErrorKind::Other | ErrorKind::SinkFailures => ErrorCategory::ExternalDependency,
            ErrorKind::RuntimeUnavailable => ErrorCategory::Unavailable,
        }
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        match self.kind {
            ErrorKind::SinkFailures => ErrorRetryAdvice::Retryable,
            ErrorKind::Other | ErrorKind::RuntimeUnavailable => ErrorRetryAdvice::DoNotRetry,
        }
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        ErrorRecord::new_freeform(self.error_code(), kind_user_text(self.kind))
            .with_category(self.error_category())
            .with_retry_advice(self.retry_advice())
            .with_freeform_diagnostic_text(self.to_string())
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        match self {
            Self {
                kind,
                repr: ErrorRepr::Other(err),
            } => {
                ErrorRecord::new_freeform(literal_error_code(kind_code(kind)), kind_user_text(kind))
                    .with_category(kind_category(kind))
                    .with_retry_advice(kind_retry_advice(kind))
                    .with_freeform_diagnostic_text(err.to_string())
            }
            Self {
                kind,
                repr: ErrorRepr::SinkFailures(failures),
            } => {
                ErrorRecord::new_freeform(literal_error_code(kind_code(kind)), kind_user_text(kind))
                    .with_category(kind_category(kind))
                    .with_retry_advice(kind_retry_advice(kind))
                    .with_freeform_diagnostic_text(format_sink_failures(&failures))
            }
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.repr {
            ErrorRepr::Other(err) => {
                if f.alternate() {
                    write!(f, "{err:#}")
                } else {
                    write!(f, "{err}")
                }
            }
            ErrorRepr::SinkFailures(failures) => {
                write!(f, "one or more sinks failed:")?;
                for failure in failures {
                    write!(f, "\n{failure}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            ErrorRepr::Other(err) => err.source(),
            ErrorRepr::SinkFailures(failures) => failures
                .first()
                .map(|failure| failure as &(dyn std::error::Error + 'static)),
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Self::new(ErrorKind::Other, ErrorRepr::Other(err))
    }
}

impl From<crate::TryNotifyError> for Error {
    fn from(err: crate::TryNotifyError) -> Self {
        Self::new(
            ErrorKind::RuntimeUnavailable,
            ErrorRepr::Other(anyhow::Error::new(err)),
        )
    }
}

#[cfg(any(
    feature = "all-sinks",
    feature = "bark",
    feature = "dingtalk",
    feature = "discord",
    feature = "feishu",
    feature = "generic-webhook",
    feature = "github",
    feature = "pushplus",
    feature = "serverchan",
    feature = "slack",
    feature = "telegram",
    feature = "wecom"
))]
impl From<http_kit::Error> for Error {
    fn from(err: http_kit::Error) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::from(anyhow::Error::from(err))
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

fn kind_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Other => "notify_kit.other",
        ErrorKind::RuntimeUnavailable => "notify_kit.runtime_unavailable",
        ErrorKind::SinkFailures => "notify_kit.sink_failures",
    }
}

fn kind_user_text(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Other => "notify-kit operation failed",
        ErrorKind::RuntimeUnavailable => "notify-kit runtime is unavailable",
        ErrorKind::SinkFailures => "one or more notification sinks failed",
    }
}

fn kind_category(kind: ErrorKind) -> ErrorCategory {
    match kind {
        ErrorKind::Other | ErrorKind::SinkFailures => ErrorCategory::ExternalDependency,
        ErrorKind::RuntimeUnavailable => ErrorCategory::Unavailable,
    }
}

fn kind_retry_advice(kind: ErrorKind) -> ErrorRetryAdvice {
    match kind {
        ErrorKind::SinkFailures => ErrorRetryAdvice::Retryable,
        ErrorKind::Other | ErrorKind::RuntimeUnavailable => ErrorRetryAdvice::DoNotRetry,
    }
}

fn format_sink_failures(failures: &[SinkFailure]) -> String {
    failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use error_kit::{ErrorCategory, ErrorRetryAdvice};
    use std::error::Error as _;

    use super::{Error, ErrorKind, SinkFailure};

    #[test]
    fn sink_failures_preserve_standard_error_source_chain() {
        let io_error = std::io::Error::other("disk full");
        let sink_error = Error::from(io_error);
        let aggregate = Error::from_sink_failures(vec![SinkFailure::new(0, "feishu", sink_error)]);

        assert_eq!(aggregate.kind(), ErrorKind::SinkFailures);

        let first = aggregate.source().expect("aggregate source should exist");
        assert_eq!(first.to_string(), "- feishu: disk full");

        let root = first
            .source()
            .expect("sink failure should expose inner error");
        assert_eq!(root.to_string(), "disk full");
    }

    #[test]
    fn runtime_unavailable_maps_to_unavailable_record() {
        let record = Error::from(crate::TryNotifyError::NoTokioRuntime).into_error_record();

        assert_eq!(record.code().as_str(), "notify_kit.runtime_unavailable");
        assert_eq!(record.category(), ErrorCategory::Unavailable);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    }

    #[test]
    fn aggregate_sink_failures_map_to_retryable_external_dependency_records() {
        let record = Error::from_sink_failures(vec![SinkFailure::new(
            0,
            "slack",
            Error::from(std::io::Error::other("dial failed")),
        )])
        .into_error_record();

        assert_eq!(record.code().as_str(), "notify_kit.sink_failures");
        assert_eq!(record.category(), ErrorCategory::ExternalDependency);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::Retryable);
        let diagnostic = format!(
            "{}",
            record
                .diagnostic_text()
                .expect("diagnostic text")
                .diagnostic_display()
        );
        assert!(diagnostic.contains("slack"));
    }
}
