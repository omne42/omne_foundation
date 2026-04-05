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

pub(crate) fn tagged_message(kind: ErrorKind, message: impl Into<String>) -> Error {
    Error::from(tag_anyhow(kind, anyhow::anyhow!(message.into())))
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    inner: anyhow::Error,
}

impl Error {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn as_anyhow(&self) -> &anyhow::Error {
        &self.inner
    }

    #[must_use]
    pub fn into_anyhow(self) -> anyhow::Error {
        self.inner
    }

    fn classify(err: &anyhow::Error) -> ErrorKind {
        if let Some(tag) = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<TaggedError>())
        {
            return tag.kind;
        }

        if let Some(reqwest) = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
        {
            return if reqwest.is_decode() {
                ErrorKind::ResponseDecode
            } else {
                ErrorKind::Transport
            };
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<serde_json::Error>().is_some())
        {
            return ErrorKind::ResponseDecode;
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
        {
            return ErrorKind::ResponseBody;
        }

        ErrorKind::Other
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{:#}", self.inner)
        } else {
            write!(f, "{}", self.inner)
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        let kind = Self::classify(&err);
        Self { kind, inner: err }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind, tag_anyhow};

    #[test]
    fn tagged_invalid_input_errors_preserve_kind() {
        let err = Error::from(tag_anyhow(
            ErrorKind::InvalidInput,
            anyhow::anyhow!("url must use https"),
        ));
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn tagged_http_status_errors_preserve_kind() {
        let err = Error::from(tag_anyhow(
            ErrorKind::HttpStatus,
            anyhow::anyhow!("http error: 403"),
        ));
        assert_eq!(err.kind(), ErrorKind::HttpStatus);
    }

    #[test]
    fn io_errors_map_to_response_body() {
        let err = Error::from(anyhow::Error::new(std::io::Error::other(
            "body read failed",
        )));
        assert_eq!(err.kind(), ErrorKind::ResponseBody);
    }
}
