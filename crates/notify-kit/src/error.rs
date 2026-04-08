#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Other,
    Config,
    Transport,
    RuntimeUnavailable,
    SinkFailures,
}

#[derive(Debug)]
struct ClassifiedError {
    kind: ErrorKind,
    source: anyhow::Error,
}

impl ClassifiedError {
    #[cfg(any(feature = "env-standard", feature = "sink-github-comment", test))]
    fn new(kind: ErrorKind, source: anyhow::Error) -> Self {
        Self { kind, source }
    }

    fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl std::fmt::Display for ClassifiedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for ClassifiedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
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

    fn classify(err: &anyhow::Error) -> ErrorKind {
        if let Some(kind) = err.chain().find_map(|cause| {
            cause
                .downcast_ref::<ClassifiedError>()
                .map(ClassifiedError::kind)
        }) {
            return kind;
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<crate::TryNotifyError>().is_some())
        {
            return ErrorKind::RuntimeUnavailable;
        }

        #[cfg(feature = "__http-stack")]
        let has_http_transport_cause = err.chain().any(|cause| {
            cause.downcast_ref::<http_kit::Error>().is_some()
                || cause.downcast_ref::<reqwest::Error>().is_some()
        });
        #[cfg(not(feature = "__http-stack"))]
        let has_http_transport_cause = false;

        if has_http_transport_cause
            || err
                .chain()
                .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
        {
            return ErrorKind::Transport;
        }

        ErrorKind::Other
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
            ErrorRepr::Other(err) => Some(err.as_ref()),
            ErrorRepr::SinkFailures(failures) => failures
                .first()
                .map(|failure| failure as &(dyn std::error::Error + 'static)),
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        let kind = Self::classify(&err);
        Self::new(kind, ErrorRepr::Other(err))
    }
}

impl From<crate::TryNotifyError> for Error {
    fn from(err: crate::TryNotifyError) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

#[cfg(feature = "__http-stack")]
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

#[cfg(any(feature = "env-standard", feature = "sink-github-comment", test))]
pub(crate) fn tagged_message(kind: ErrorKind, message: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(ClassifiedError::new(kind, anyhow::anyhow!("{message}")))
}

#[cfg(any(feature = "env-standard", feature = "sink-github-comment", test))]
pub(crate) fn wrap_kind(kind: ErrorKind, err: anyhow::Error) -> anyhow::Error {
    if err
        .chain()
        .any(|cause| cause.downcast_ref::<ClassifiedError>().is_some())
    {
        err
    } else {
        anyhow::Error::new(ClassifiedError::new(kind, err))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::{Error, ErrorKind, SinkFailure, tagged_message, wrap_kind};

    #[test]
    fn tagged_errors_keep_stable_config_kind() {
        let err = Error::from(tagged_message(ErrorKind::Config, "invalid notify config"));
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[test]
    fn http_errors_classify_as_transport() {
        let err = Error::from(anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "dial failed",
        )));
        assert_eq!(err.kind(), ErrorKind::Transport);
    }

    #[test]
    fn wrapped_errors_preserve_explicit_kind() {
        let err = Error::from(wrap_kind(ErrorKind::Config, anyhow::anyhow!("bad env")));
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[test]
    fn sink_failure_exposes_underlying_error_as_source() {
        let failure = SinkFailure::new(
            2,
            "bad",
            Error::from(anyhow::Error::new(std::io::Error::other("dial failed"))),
        );

        let source = failure.source().expect("sink failure source");
        assert_eq!(source.to_string(), "dial failed");
    }

    #[test]
    fn aggregate_sink_failures_expose_first_failure_via_source() {
        let err = Error::from_sink_failures(vec![
            SinkFailure::new(
                1,
                "bad",
                Error::from(anyhow::Error::new(std::io::Error::other("dial failed"))),
            ),
            SinkFailure::new(3, "other", Error::from(anyhow::anyhow!("boom"))),
        ]);

        let source = err.source().expect("aggregate source");
        assert_eq!(source.to_string(), "- bad: dial failed");
        let nested = source.source().expect("nested error");
        assert_eq!(nested.to_string(), "dial failed");
    }

    #[test]
    fn wrapped_anyhow_error_remains_visible_via_source() {
        let err = Error::from(wrap_kind(
            ErrorKind::Config,
            anyhow::Error::new(std::io::Error::other("bad env")),
        ));

        let source = err.source().expect("wrapped anyhow source");
        assert_eq!(source.to_string(), "bad env");
        let nested = source.source().expect("nested io source");
        assert_eq!(nested.to_string(), "bad env");
    }
}
