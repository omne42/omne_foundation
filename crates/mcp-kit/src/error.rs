use std::fmt;

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
pub struct Error {
    kind: ErrorKind,
    inner: anyhow::Error,
}

#[derive(Debug)]
struct KindTaggedError {
    kind: ErrorKind,
    source: anyhow::Error,
}

impl fmt::Display for KindTaggedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(f)
    }
}

impl std::error::Error for KindTaggedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
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

    fn tagged(kind: ErrorKind, err: anyhow::Error) -> Self {
        Self {
            kind,
            inner: anyhow::Error::new(KindTaggedError { kind, source: err }),
        }
    }

    pub(crate) fn config(err: anyhow::Error) -> Self {
        Self::tagged(ErrorKind::Config, err)
    }

    pub(crate) fn manager_state(err: anyhow::Error) -> Self {
        Self::tagged(ErrorKind::ManagerState, err)
    }

    pub(crate) fn config_anyhow(err: anyhow::Error) -> anyhow::Error {
        Self::config(err).into_anyhow()
    }

    pub(crate) fn manager_state_anyhow(err: anyhow::Error) -> anyhow::Error {
        Self::manager_state(err).into_anyhow()
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

    fn classify(err: &anyhow::Error) -> ErrorKind {
        if let Some(kind) = err.chain().find_map(|cause| {
            cause
                .downcast_ref::<KindTaggedError>()
                .map(|tagged| tagged.kind)
        }) {
            return kind;
        }

        if err.chain().any(|cause| {
            cause
                .downcast_ref::<mcp_jsonrpc::Error>()
                .is_some_and(mcp_jsonrpc::Error::is_wait_timeout)
        }) {
            return ErrorKind::Timeout;
        }

        if err
            .chain()
            .any(|cause| cause.downcast_ref::<mcp_jsonrpc::Error>().is_some())
        {
            return ErrorKind::Protocol;
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

        let mut chain_text = String::new();
        for cause in err.chain() {
            if !chain_text.is_empty() {
                chain_text.push_str(" | ");
            }
            chain_text.push_str(&cause.to_string());
        }
        let chain_text = chain_text.to_ascii_lowercase();

        if chain_text.contains("not connected")
            || chain_text.contains("cannot be reused for cwd")
            || chain_text.contains("reentrantly")
            || chain_text.contains("became unavailable before")
        {
            return ErrorKind::ManagerState;
        }

        if chain_text.contains("mcp config")
            || chain_text.contains("invalid mcp server config")
            || chain_text.contains("unknown mcp server")
            || chain_text.contains("override config path")
            || chain_text.contains("transport=")
            || chain_text.contains("stdout_log")
            || chain_text.contains("client.protocol_version")
            || chain_text.contains("client.capabilities")
            || chain_text.contains("client.roots")
            || chain_text.contains("bearer_token_env_var")
            || chain_text.contains("unix_path")
            || chain_text.contains("untrusted mode")
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

    use super::{Error, ErrorKind};

    #[test]
    fn classifies_config_errors() {
        let err = Error::config(anyhow!("mcp config not found under root /tmp"));
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
        let err = Error::manager_state(anyhow!("mcp server not connected: demo"));
        assert_eq!(err.kind(), ErrorKind::ManagerState);
    }

    #[test]
    fn context_preserves_explicit_kind() {
        let err = Error::config(anyhow!("mcp config not found")).context("load demo config");
        assert_eq!(err.kind(), ErrorKind::Config);
    }
}
