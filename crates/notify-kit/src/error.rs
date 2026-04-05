#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Other,
    Config,
    Transport,
    Io,
    InvalidResponse,
    RuntimeUnavailable,
    SinkFailures,
}

#[derive(Debug)]
pub(crate) struct TaggedError {
    kind: ErrorKind,
    source: anyhow::Error,
}

impl TaggedError {
    fn new(kind: ErrorKind, source: anyhow::Error) -> Self {
        Self { kind, source }
    }
}

impl std::fmt::Display for TaggedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for TaggedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn tag_anyhow(kind: ErrorKind, err: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(TaggedError::new(kind, err))
}

pub(crate) fn tagged_message(kind: ErrorKind, message: impl Into<String>) -> anyhow::Error {
    tag_anyhow(kind, anyhow::anyhow!(message.into()))
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
        if let Some(tag) = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<TaggedError>())
        {
            return tag.kind;
        }

        #[cfg(feature = "http-client")]
        if err
            .chain()
            .any(|cause| cause.downcast_ref::<http_kit::Error>().is_some())
        {
            return ErrorKind::Transport;
        }

        #[cfg(feature = "http-client")]
        if err
            .chain()
            .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some())
        {
            return ErrorKind::Transport;
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<serde_json::Error>().is_some())
        {
            return ErrorKind::InvalidResponse;
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
        {
            return ErrorKind::Io;
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
            ErrorRepr::Other(err) => err.source(),
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
        Self::new(
            ErrorKind::RuntimeUnavailable,
            ErrorRepr::Other(anyhow::Error::new(err)),
        )
    }
}

#[cfg(feature = "http-client")]
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

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind, SinkFailure, tagged_message};

    #[test]
    fn sink_failure_exposes_inner_error_as_source() {
        let failure = SinkFailure::new(
            0,
            "slack",
            Error::from(std::io::Error::other("network failed")),
        );
        let source = std::error::Error::source(&failure).expect("source");
        assert!(source.to_string().contains("network failed"), "{source}");
    }

    #[test]
    fn aggregate_error_uses_first_failure_for_source_chain() {
        let aggregated = Error::from_sink_failures(vec![
            SinkFailure::new(
                1,
                "slack",
                Error::from(std::io::Error::other("first failure")),
            ),
            SinkFailure::new(
                2,
                "feishu",
                Error::from(std::io::Error::other("second failure")),
            ),
        ]);

        let source = std::error::Error::source(&aggregated).expect("source");
        assert!(source.to_string().contains("slack"), "{source}");
        let nested = std::error::Error::source(source).expect("nested source");
        assert!(nested.to_string().contains("first failure"), "{nested}");
    }

    #[test]
    fn tagged_config_error_preserves_kind() {
        let err = Error::from(tagged_message(ErrorKind::Config, "bad config"));
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[cfg(feature = "http-client")]
    #[test]
    fn http_error_maps_to_transport_kind() {
        let err = Error::from(http_kit::Error::from(anyhow::anyhow!("transport failed")));
        assert_eq!(err.kind(), ErrorKind::Transport);
    }
}
