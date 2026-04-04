#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Other,
    Config,
    Connection,
    Protocol,
    Timeout,
    ManagerState,
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

    fn kind(&self) -> ErrorKind {
        self.kind
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
pub struct Error {
    kind: ErrorKind,
    inner: anyhow::Error,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn into_anyhow(self) -> anyhow::Error {
        self.inner
    }

    #[must_use]
    pub fn as_anyhow(&self) -> &anyhow::Error {
        &self.inner
    }

    pub fn chain(&self) -> anyhow::Chain<'_> {
        self.inner.chain()
    }

    #[must_use]
    pub fn context(self, context: impl std::fmt::Display + Send + Sync + 'static) -> Self {
        Self {
            kind: self.kind,
            inner: self.inner.context(context),
        }
    }

    #[must_use]
    pub fn with_context<C, F>(self, f: F) -> Self
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        Self {
            kind: self.kind,
            inner: self.inner.context(f()),
        }
    }

    fn classify_mcp_jsonrpc_error(err: &mcp_jsonrpc::Error) -> ErrorKind {
        match err {
            mcp_jsonrpc::Error::Io(_) => ErrorKind::Connection,
            mcp_jsonrpc::Error::Protocol(protocol)
                if protocol.kind == mcp_jsonrpc::ProtocolErrorKind::WaitTimeout =>
            {
                ErrorKind::Timeout
            }
            mcp_jsonrpc::Error::Json(_)
            | mcp_jsonrpc::Error::Rpc { .. }
            | mcp_jsonrpc::Error::Protocol(_) => ErrorKind::Protocol,
        }
    }

    fn classify(err: &anyhow::Error) -> ErrorKind {
        if let Some(tag) = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<TaggedError>())
        {
            return tag.kind();
        }

        if err.chain().any(|cause| {
            cause
                .downcast_ref::<mcp_jsonrpc::Error>()
                .is_some_and(mcp_jsonrpc::Error::is_wait_timeout)
        }) {
            return ErrorKind::Timeout;
        }

        if let Some(kind) = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<mcp_jsonrpc::Error>())
            .map(Self::classify_mcp_jsonrpc_error)
        {
            return kind;
        }

        if err.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>().is_some()
                || cause.downcast_ref::<reqwest::Error>().is_some()
                || cause.downcast_ref::<http_kit::Error>().is_some()
        }) {
            return ErrorKind::Connection;
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<crate::ServerNameError>().is_some())
        {
            return ErrorKind::Config;
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

impl From<http_kit::Error> for Error {
    fn from(err: http_kit::Error) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

impl From<mcp_jsonrpc::Error> for Error {
    fn from(err: mcp_jsonrpc::Error) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

impl From<crate::ServerNameError> for Error {
    fn from(err: crate::ServerNameError) -> Self {
        Self::from(anyhow::Error::new(err))
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::{Error, ErrorKind, tag_anyhow, tagged_message};

    #[test]
    fn classifies_config_errors() {
        let err = Error::from(tagged_message(
            ErrorKind::Config,
            "mcp config not found under root /tmp",
        ));
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[test]
    fn classifies_connection_errors() {
        let err = Error::from(anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "dial failed",
        )));
        assert_eq!(err.kind(), ErrorKind::Connection);
    }

    #[test]
    fn classifies_wrapped_jsonrpc_io_errors_as_connection_errors() {
        let err = Error::from(mcp_jsonrpc::Error::from(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "transport closed",
        )));
        assert_eq!(err.kind(), ErrorKind::Connection);
    }

    #[test]
    fn classifies_protocol_errors() {
        let err = Error::from(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::InvalidInput,
            "bad request",
        ));
        assert_eq!(err.kind(), ErrorKind::Protocol);
    }

    #[test]
    fn classifies_timeout_errors() {
        let err = Error::from(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::WaitTimeout,
            "timed out",
        ));
        assert_eq!(err.kind(), ErrorKind::Timeout);
    }

    #[test]
    fn classifies_manager_state_errors() {
        let err = Error::from(tagged_message(
            ErrorKind::ManagerState,
            "mcp server not connected: demo",
        ));
        assert_eq!(err.kind(), ErrorKind::ManagerState);
    }

    #[test]
    fn preserves_tagged_error_kind_through_context() {
        let err = Error::from(tag_anyhow(
            ErrorKind::Config,
            anyhow!("invalid client.capabilities"),
        ))
        .context("wrap context");
        assert_eq!(err.kind(), ErrorKind::Config);
    }
}
