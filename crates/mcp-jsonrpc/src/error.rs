use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};
use serde_json::Value;
use structured_text_kit::StructuredText;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("json-rpc error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("protocol error: {0}")]
    Protocol(ProtocolError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtocolErrorKind {
    /// The client/transport was closed (explicitly or via drop).
    Closed,
    /// Waiting for a child process to exit timed out.
    WaitTimeout,
    /// The peer sent an invalid JSON / JSON-RPC message.
    InvalidMessage,
    /// Invalid user input (e.g. invalid header name/value).
    InvalidInput,
    /// Streamable HTTP transport error (SSE/POST bridge).
    StreamableHttp,
    /// Catch-all for internal invariants.
    Other,
}

#[derive(Debug, Clone)]
pub struct ProtocolError {
    pub kind: ProtocolErrorKind,
    pub message: String,
}

impl ProtocolError {
    pub fn new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for ProtocolError {}

impl Error {
    pub fn protocol(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        Self::Protocol(ProtocolError::new(kind, message))
    }

    /// Returns true if this error was produced by `Client::wait_with_timeout`.
    pub fn is_wait_timeout(&self) -> bool {
        matches!(self, Error::Protocol(err) if err.kind == ProtocolErrorKind::WaitTimeout)
    }

    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::Io(err) => io_error_code(err.kind()),
            Self::Json(_) => literal_error_code("mcp_jsonrpc.json"),
            Self::Rpc { code, .. } => rpc_error_code(*code),
            Self::Protocol(err) => protocol_error_code(err.kind),
        }
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        match self {
            Self::Io(err) => io_error_category(err.kind()),
            Self::Json(_) => ErrorCategory::ExternalDependency,
            Self::Rpc { code, .. } => rpc_error_category(*code),
            Self::Protocol(err) => protocol_error_category(err.kind),
        }
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        match self {
            Self::Io(err) => io_error_retry_advice(err.kind()),
            Self::Json(_) => ErrorRetryAdvice::DoNotRetry,
            Self::Rpc { code, .. } => rpc_error_retry_advice(*code),
            Self::Protocol(err) => protocol_error_retry_advice(err.kind),
        }
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        match self {
            Self::Io(source) => build_io_error_record(source.kind(), source.to_string()),
            Self::Json(source) => build_json_error_record(source.to_string()),
            Self::Rpc {
                code,
                message,
                data,
            } => build_rpc_error_record(*code, message.as_str(), data.is_some()),
            Self::Protocol(err) => build_protocol_error_record(err.kind, err.message.as_str()),
        }
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        match self {
            Self::Io(source) => {
                build_io_error_record(source.kind(), source.to_string()).with_source(source)
            }
            Self::Json(source) => build_json_error_record(source.to_string()).with_source(source),
            Self::Rpc {
                code,
                message,
                data,
            } => build_rpc_error_record(code, message.as_str(), data.is_some()),
            Self::Protocol(err) => build_protocol_error_record(err.kind, err.message.as_str()),
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

fn build_io_error_record(kind: std::io::ErrorKind, detail: String) -> ErrorRecord {
    ErrorRecord::new(io_error_code(kind), io_error_user_text(kind))
        .with_category(io_error_category(kind))
        .with_retry_advice(io_error_retry_advice(kind))
        .with_diagnostic_text(StructuredText::freeform(format!(
            "json-rpc transport io error: {detail}"
        )))
}

fn io_error_code(kind: std::io::ErrorKind) -> ErrorCode {
    match kind {
        std::io::ErrorKind::NotFound => literal_error_code("mcp_jsonrpc.io.not_found"),
        std::io::ErrorKind::PermissionDenied => {
            literal_error_code("mcp_jsonrpc.io.permission_denied")
        }
        std::io::ErrorKind::TimedOut => literal_error_code("mcp_jsonrpc.io.timeout"),
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            literal_error_code("mcp_jsonrpc.io.invalid_input")
        }
        _ => literal_error_code("mcp_jsonrpc.io"),
    }
}

fn io_error_category(kind: std::io::ErrorKind) -> ErrorCategory {
    match kind {
        std::io::ErrorKind::NotFound => ErrorCategory::NotFound,
        std::io::ErrorKind::PermissionDenied => ErrorCategory::PermissionDenied,
        std::io::ErrorKind::TimedOut => ErrorCategory::Timeout,
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            ErrorCategory::InvalidInput
        }
        _ => ErrorCategory::ExternalDependency,
    }
}

fn io_error_retry_advice(kind: std::io::ErrorKind) -> ErrorRetryAdvice {
    match kind {
        std::io::ErrorKind::NotFound
        | std::io::ErrorKind::PermissionDenied
        | std::io::ErrorKind::InvalidInput
        | std::io::ErrorKind::InvalidData => ErrorRetryAdvice::DoNotRetry,
        std::io::ErrorKind::TimedOut => ErrorRetryAdvice::Retryable,
        _ => ErrorRetryAdvice::Retryable,
    }
}

fn io_error_user_text(kind: std::io::ErrorKind) -> StructuredText {
    StructuredText::freeform(match kind {
        std::io::ErrorKind::NotFound => "json-rpc transport resource not found",
        std::io::ErrorKind::PermissionDenied => "json-rpc transport permission denied",
        std::io::ErrorKind::TimedOut => "json-rpc transport timed out",
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            "json-rpc transport rejected invalid input"
        }
        _ => "json-rpc transport io error",
    })
}

fn build_json_error_record(detail: String) -> ErrorRecord {
    ErrorRecord::new(
        literal_error_code("mcp_jsonrpc.json"),
        StructuredText::freeform("json-rpc json processing failed"),
    )
    .with_category(ErrorCategory::ExternalDependency)
    .with_retry_advice(ErrorRetryAdvice::DoNotRetry)
    .with_diagnostic_text(StructuredText::freeform(format!(
        "json-rpc json error: {detail}"
    )))
}

fn build_rpc_error_record(code: i64, message: &str, has_data: bool) -> ErrorRecord {
    let suffix = if has_data { "; data_present=true" } else { "" };
    ErrorRecord::new(rpc_error_code(code), rpc_error_user_text(code))
        .with_category(rpc_error_category(code))
        .with_retry_advice(rpc_error_retry_advice(code))
        .with_diagnostic_text(StructuredText::freeform(format!(
            "remote json-rpc error {code}: {message}{suffix}"
        )))
}

fn rpc_error_code(code: i64) -> ErrorCode {
    match code {
        -32700 => literal_error_code("mcp_jsonrpc.rpc.parse_error"),
        -32600 => literal_error_code("mcp_jsonrpc.rpc.invalid_request"),
        -32601 => literal_error_code("mcp_jsonrpc.rpc.method_not_found"),
        -32602 => literal_error_code("mcp_jsonrpc.rpc.invalid_params"),
        -32603 => literal_error_code("mcp_jsonrpc.rpc.internal_error"),
        -32800 => literal_error_code("mcp_jsonrpc.rpc.request_cancelled"),
        -32801 => literal_error_code("mcp_jsonrpc.rpc.content_modified"),
        -32099..=-32000 => literal_error_code("mcp_jsonrpc.rpc.server_error"),
        _ => literal_error_code("mcp_jsonrpc.rpc"),
    }
}

fn rpc_error_category(code: i64) -> ErrorCategory {
    match code {
        -32700 | -32600 | -32602 => ErrorCategory::InvalidInput,
        -32601 => ErrorCategory::NotFound,
        -32800 | -32801 => ErrorCategory::Conflict,
        -32603 | -32099..=-32000 => ErrorCategory::ExternalDependency,
        _ => ErrorCategory::ExternalDependency,
    }
}

fn rpc_error_retry_advice(code: i64) -> ErrorRetryAdvice {
    match code {
        -32700 | -32600 | -32601 | -32602 => ErrorRetryAdvice::DoNotRetry,
        -32800 | -32801 | -32603 | -32099..=-32000 => ErrorRetryAdvice::Retryable,
        _ => ErrorRetryAdvice::DoNotRetry,
    }
}

fn rpc_error_user_text(code: i64) -> StructuredText {
    StructuredText::freeform(match code {
        -32700 => "remote json-rpc parse error",
        -32600 => "remote json-rpc request was invalid",
        -32601 => "remote json-rpc method not found",
        -32602 => "remote json-rpc parameters were invalid",
        -32603 => "remote json-rpc internal error",
        -32800 => "remote json-rpc request was cancelled",
        -32801 => "remote json-rpc content was modified",
        -32099..=-32000 => "remote json-rpc server error",
        _ => "remote json-rpc call failed",
    })
}

fn build_protocol_error_record(kind: ProtocolErrorKind, message: &str) -> ErrorRecord {
    ErrorRecord::new(protocol_error_code(kind), protocol_error_user_text(kind))
        .with_category(protocol_error_category(kind))
        .with_retry_advice(protocol_error_retry_advice(kind))
        .with_diagnostic_text(StructuredText::freeform(message))
}

fn protocol_error_code(kind: ProtocolErrorKind) -> ErrorCode {
    match kind {
        ProtocolErrorKind::Closed => literal_error_code("mcp_jsonrpc.protocol.closed"),
        ProtocolErrorKind::WaitTimeout => literal_error_code("mcp_jsonrpc.protocol.wait_timeout"),
        ProtocolErrorKind::InvalidMessage => {
            literal_error_code("mcp_jsonrpc.protocol.invalid_message")
        }
        ProtocolErrorKind::InvalidInput => literal_error_code("mcp_jsonrpc.protocol.invalid_input"),
        ProtocolErrorKind::StreamableHttp => {
            literal_error_code("mcp_jsonrpc.protocol.streamable_http")
        }
        ProtocolErrorKind::Other => literal_error_code("mcp_jsonrpc.protocol.other"),
    }
}

fn protocol_error_category(kind: ProtocolErrorKind) -> ErrorCategory {
    match kind {
        ProtocolErrorKind::Closed => ErrorCategory::Unavailable,
        ProtocolErrorKind::WaitTimeout => ErrorCategory::Timeout,
        ProtocolErrorKind::InvalidMessage => ErrorCategory::ExternalDependency,
        ProtocolErrorKind::InvalidInput => ErrorCategory::InvalidInput,
        ProtocolErrorKind::StreamableHttp => ErrorCategory::ExternalDependency,
        ProtocolErrorKind::Other => ErrorCategory::Internal,
    }
}

fn protocol_error_retry_advice(kind: ProtocolErrorKind) -> ErrorRetryAdvice {
    match kind {
        ProtocolErrorKind::Closed
        | ProtocolErrorKind::WaitTimeout
        | ProtocolErrorKind::StreamableHttp => ErrorRetryAdvice::Retryable,
        ProtocolErrorKind::InvalidMessage
        | ProtocolErrorKind::InvalidInput
        | ProtocolErrorKind::Other => ErrorRetryAdvice::DoNotRetry,
    }
}

fn protocol_error_user_text(kind: ProtocolErrorKind) -> StructuredText {
    StructuredText::freeform(match kind {
        ProtocolErrorKind::Closed => "json-rpc client closed",
        ProtocolErrorKind::WaitTimeout => "json-rpc wait timed out",
        ProtocolErrorKind::InvalidMessage => "json-rpc peer sent invalid message",
        ProtocolErrorKind::InvalidInput => "json-rpc input was invalid",
        ProtocolErrorKind::StreamableHttp => "json-rpc streamable http transport failed",
        ProtocolErrorKind::Other => "json-rpc protocol error",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_wait_timeout_maps_to_retryable_timeout_record() {
        let err = Error::protocol(ProtocolErrorKind::WaitTimeout, "wait timed out after 1s");

        let record = err.error_record();

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.protocol.wait_timeout");
        assert_eq!(record.category(), ErrorCategory::Timeout);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::Retryable);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("json-rpc wait timed out")
        );
        assert_eq!(
            record
                .diagnostic_text()
                .and_then(StructuredText::freeform_text),
            Some("wait timed out after 1s")
        );
    }

    #[test]
    fn rpc_method_not_found_maps_to_not_found_record() {
        let err = Error::Rpc {
            code: -32601,
            message: String::from("tools/list"),
            data: None,
        };

        let record = err.error_record();

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.rpc.method_not_found");
        assert_eq!(record.category(), ErrorCategory::NotFound);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("remote json-rpc method not found")
        );
        assert_eq!(
            record
                .diagnostic_text()
                .and_then(StructuredText::freeform_text),
            Some("remote json-rpc error -32601: tools/list")
        );
    }

    #[test]
    fn into_error_record_preserves_io_source() {
        let err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ));

        let record = err.into_error_record();

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.io.permission_denied");
        assert_eq!(record.category(), ErrorCategory::PermissionDenied);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("json-rpc transport permission denied")
        );
        assert_eq!(
            record
                .source_ref()
                .expect("io source should be preserved")
                .to_string(),
            "permission denied"
        );
    }
}
