#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Transport,
    ResponseBody,
    ResponseDecode,
    HttpStatus,
    Other,
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn with_source<E>(kind: ErrorKind, message: impl Into<String>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

pub(crate) fn tagged_message(kind: ErrorKind, message: impl Into<String>) -> Error {
    Error::new(kind, message)
}

pub(crate) fn tagged_source<E>(kind: ErrorKind, message: impl Into<String>, source: E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    Error::with_source(kind, message, source)
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::{Error, ErrorKind, tagged_message, tagged_source};

    #[test]
    fn tagged_invalid_input_errors_preserve_kind() {
        let err = tagged_message(ErrorKind::InvalidInput, "url must use https");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(err.message(), "url must use https");
    }

    #[test]
    fn tagged_http_status_errors_preserve_kind() {
        let err = tagged_message(ErrorKind::HttpStatus, "http error: 403");
        assert_eq!(err.kind(), ErrorKind::HttpStatus);
        assert_eq!(err.message(), "http error: 403");
    }

    #[test]
    fn source_errors_remain_available_without_anyhow() {
        let err = tagged_source(
            ErrorKind::ResponseBody,
            "body read failed",
            std::io::Error::other("permission denied"),
        );
        assert_eq!(err.kind(), ErrorKind::ResponseBody);
        assert_eq!(err.message(), "body read failed");
        let source = err.source().expect("source");
        assert_eq!(source.to_string(), "permission denied");
    }

    #[test]
    fn display_stays_at_message_boundary() {
        let err = Error::with_source(
            ErrorKind::Transport,
            "request failed",
            std::io::Error::other("connection reset"),
        );
        assert_eq!(err.to_string(), "request failed");
    }
}
