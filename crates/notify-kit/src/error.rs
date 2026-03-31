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

#[cfg(test)]
mod tests {
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
}
