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
struct ClassifiedError {
    kind: ErrorKind,
    source: anyhow::Error,
}

impl ClassifiedError {
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
        Self::from(self.inner.context(context))
    }

    #[must_use]
    pub fn with_context<C, F>(self, f: F) -> Self
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        Self::from(self.inner.context(f()))
    }

    fn classify(err: &anyhow::Error) -> ErrorKind {
        if let Some(kind) = err.chain().find_map(|cause| {
            cause
                .downcast_ref::<ClassifiedError>()
                .map(ClassifiedError::kind)
        }) {
            return kind;
        }

        if let Some(kind) = err.chain().find_map(|cause| {
            cause
                .downcast_ref::<mcp_jsonrpc::Error>()
                .map(classify_jsonrpc_error)
        }) {
            return kind;
        }

        if err.chain().any(|cause| {
            cause.downcast_ref::<crate::ServerNameError>().is_some()
                || cause.downcast_ref::<config_kit::Error>().is_some()
        }) {
            return ErrorKind::Config;
        }

        if err.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>().is_some()
                || cause.downcast_ref::<reqwest::Error>().is_some()
                || cause.downcast_ref::<http_kit::Error>().is_some()
        }) {
            return ErrorKind::Connection;
        }

        ErrorKind::Other
    }
}

fn classify_jsonrpc_error(err: &mcp_jsonrpc::Error) -> ErrorKind {
    match err {
        mcp_jsonrpc::Error::Io(_) => ErrorKind::Connection,
        _ if err.is_wait_timeout() => ErrorKind::Timeout,
        mcp_jsonrpc::Error::Protocol(protocol)
            if matches!(
                protocol.kind,
                mcp_jsonrpc::ProtocolErrorKind::Closed
                    | mcp_jsonrpc::ProtocolErrorKind::StreamableHttp
            ) =>
        {
            ErrorKind::Connection
        }
        mcp_jsonrpc::Error::Json(_)
        | mcp_jsonrpc::Error::Rpc { .. }
        | mcp_jsonrpc::Error::Protocol(_) => ErrorKind::Protocol,
    }
}

pub(crate) fn tagged_message(kind: ErrorKind, message: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(ClassifiedError::new(kind, anyhow::anyhow!("{message}")))
}

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
    use super::{Error, ErrorKind};

    #[test]
    fn classifies_config_errors() {
        let err = Error::from(super::tagged_message(
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
    fn classifies_jsonrpc_io_errors_as_connection_errors() {
        let err = Error::from(mcp_jsonrpc::Error::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "write failed",
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
    fn classifies_jsonrpc_closed_errors_as_connection_errors() {
        let err = Error::from(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::Closed,
            "transport closed",
        ));
        assert_eq!(err.kind(), ErrorKind::Connection);
    }

    #[test]
    fn classifies_streamable_http_errors_as_connection_errors() {
        let err = Error::from(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::StreamableHttp,
            "http bridge failed",
        ));
        assert_eq!(err.kind(), ErrorKind::Connection);
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
        let err = Error::from(super::tagged_message(
            ErrorKind::ManagerState,
            "mcp server not connected: demo",
        ));
        assert_eq!(err.kind(), ErrorKind::ManagerState);
    }

    #[test]
    fn wrapped_errors_keep_explicit_kind() {
        let err = Error::from(super::wrap_kind(
            ErrorKind::Config,
            anyhow::anyhow!("invalid mcp server config"),
        ));
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[test]
    fn preserves_wrapped_kind_through_anyhow_context_before_conversion() {
        let err = super::wrap_kind(
            ErrorKind::ManagerState,
            anyhow::anyhow!("server already connected"),
        )
        .context("outer anyhow context")
        .context("second anyhow context");

        assert_eq!(Error::from(err).kind(), ErrorKind::ManagerState);
    }

    #[test]
    fn classifies_typed_jsonrpc_errors_through_anyhow_context_layers() {
        let err = anyhow::Error::new(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::WaitTimeout,
            "timed out",
        ))
        .context("close session");

        assert_eq!(Error::from(err).kind(), ErrorKind::Timeout);
    }

    #[test]
    fn classifies_config_kit_errors_without_message_matching() {
        let missing = std::env::temp_dir().join("mcp-kit-missing-config.json");
        let err = config_kit::load_config_document(&missing, config_kit::ConfigLoadOptions::new())
            .expect_err("missing document should fail");
        let err = Error::from(anyhow::Error::new(err));
        assert_eq!(err.kind(), ErrorKind::Config);
    }
}
