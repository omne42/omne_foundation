#![forbid(unsafe_code)]

//! `mcp-jsonrpc` is a small JSON-RPC 2.0 client with a few MCP-friendly transports.
//!
//! Transports:
//! - stdio (spawned child process)
//! - unix domain socket (connect to an existing local server)
//! - "streamable http" (HTTP SSE + POST), commonly used by remote MCP servers
//!   - Redirects are disabled by default (opt in via `StreamableHttpOptions.follow_redirects`).
//!
//! Design goals:
//! - Minimal dependencies and low ceremony (`serde_json::Value` based)
//! - Support both notifications and server->client requests
//! - Bounded queues + per-message size limits to reduce DoS risk
//!
//! Non-goals:
//! - Implementing a JSON-RPC server
//! - Automatic reconnect
//! - Rich typed schemas beyond `serde_json::Value`

use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc as std_mpsc};
use std::time::Duration;

use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use structured_text_kit::StructuredText;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

mod stdout_log;
mod streamable_http;

use stdout_log::LogState;

pub type StdoutLogRedactor = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const REUSABLE_LINE_BUFFER_RETAIN_BYTES: usize = 64 * 1024;
const READ_LINE_INITIAL_CAP_BYTES: usize = 4 * 1024;
const TOKIO_TIME_DRIVER_ERROR: &str =
    "tokio runtime time driver is not enabled; build the runtime with enable_time()";
type DetachedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub(crate) fn ensure_tokio_time_driver(operation: &'static str) -> Result<(), Error> {
    std::panic::catch_unwind(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    })
    .map_err(|_| {
        Error::protocol(
            ProtocolErrorKind::Other,
            format!("{TOKIO_TIME_DRIVER_ERROR} ({operation})"),
        )
    })
}

#[derive(Clone)]
pub struct SpawnOptions {
    pub stdout_log: Option<StdoutLog>,
    /// Optional transformation applied to each captured stdout line before it is written to
    /// `stdout_log`.
    ///
    /// This can be used to redact secrets before they are written to disk.
    pub stdout_log_redactor: Option<StdoutLogRedactor>,
    pub limits: Limits,
    pub diagnostics: DiagnosticsOptions,
    /// When true (default), kill the child process if the `Client` is dropped.
    ///
    /// Note: this is best-effort and does not guarantee the child is reaped. Prefer an explicit
    /// `Client::wait*` call when you own the child lifecycle.
    pub kill_on_drop: bool,
}

impl std::fmt::Debug for SpawnOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnOptions")
            .field("stdout_log", &self.stdout_log)
            .field("stdout_log_redactor", &self.stdout_log_redactor.is_some())
            .field("limits", &self.limits)
            .field("diagnostics", &self.diagnostics)
            .field("kill_on_drop", &self.kill_on_drop)
            .finish()
    }
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            stdout_log: None,
            stdout_log_redactor: None,
            limits: Limits::default(),
            diagnostics: DiagnosticsOptions::default(),
            kill_on_drop: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamableHttpOptions {
    /// Extra HTTP headers to include on all requests.
    pub headers: HashMap<String, String>,
    /// Whether untrusted transports must pin the validated public IP set into the actual socket.
    pub enforce_public_ip: bool,
    /// Optional timeout applied while establishing HTTP connections.
    pub connect_timeout: Option<Duration>,
    /// Optional timeout applied to individual HTTP POST request/response bodies.
    ///
    /// Note: do not use this to limit the long-lived SSE connection.
    pub request_timeout: Option<Duration>,
    /// Whether to follow HTTP redirects (default: false).
    ///
    /// For safety, the default is to disable redirects to reduce SSRF risk.
    pub follow_redirects: bool,
    /// Maximum bytes of HTTP response body to include in bridged JSON-RPC error data.
    ///
    /// Default: 0 (do not include body previews) to reduce accidental secrets exposure.
    pub error_body_preview_bytes: usize,
}

impl Default for StreamableHttpOptions {
    fn default() -> Self {
        Self {
            headers: HashMap::new(),
            enforce_public_ip: false,
            connect_timeout: Some(Duration::from_secs(10)),
            request_timeout: None,
            follow_redirects: false,
            error_body_preview_bytes: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticsOptions {
    /// Capture up to N invalid JSON lines (best-effort) for debugging.
    ///
    /// Default: 0 (disabled).
    pub invalid_json_sample_lines: usize,
    /// Maximum bytes per captured invalid JSON line.
    ///
    /// Default: 256.
    pub invalid_json_sample_max_bytes: usize,
}

impl Default for DiagnosticsOptions {
    fn default() -> Self {
        Self {
            invalid_json_sample_lines: 0,
            invalid_json_sample_max_bytes: 256,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StdoutLog {
    pub path: PathBuf,
    pub max_bytes_per_part: u64,
    /// Keep at most N rotated parts (`*.segment-XXXX.log`). When `None`, keep all.
    pub max_parts: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Limits {
    /// Maximum bytes for a single JSON-RPC message (one line).
    pub max_message_bytes: usize,
    /// Maximum buffered notifications from the server.
    pub notifications_capacity: usize,
    /// Maximum buffered server->client requests.
    pub requests_capacity: usize,
    /// Maximum in-flight client->server requests waiting for responses.
    pub max_pending_requests: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Large enough for typical MCP messages, but bounded to reduce DoS risk.
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            notifications_capacity: 256,
            requests_capacity: 64,
            max_pending_requests: 64,
        }
    }
}

pub(crate) fn normalize_max_message_bytes(max_message_bytes: usize) -> usize {
    if max_message_bytes == 0 {
        return DEFAULT_MAX_MESSAGE_BYTES;
    }
    max_message_bytes
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Id {
    String(String),
    Integer(i64),
    Unsigned(u64),
}

type PendingRequests = Arc<Mutex<HashMap<Id, oneshot::Sender<Result<Value, Error>>>>>;
type CancelledRequestIds = Arc<Mutex<CancelledRequestIdsState>>;

const CANCELLED_REQUEST_IDS_MAX: usize = 1024;

#[derive(Debug, Default)]
struct CancelledRequestIdsState {
    order: VecDeque<(u64, Id)>,
    latest: HashMap<Id, u64>,
    next_generation: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientStats {
    pub invalid_json_lines: u64,
    pub dropped_notifications_full: u64,
    pub dropped_notifications_closed: u64,
}

#[derive(Debug, Default)]
struct ClientStatsInner {
    invalid_json_lines: AtomicU64,
    dropped_notifications_full: AtomicU64,
    dropped_notifications_closed: AtomicU64,
}

impl ClientStatsInner {
    fn snapshot(&self) -> ClientStats {
        ClientStats {
            invalid_json_lines: self.invalid_json_lines.load(Ordering::Relaxed),
            dropped_notifications_full: self.dropped_notifications_full.load(Ordering::Relaxed),
            dropped_notifications_closed: self.dropped_notifications_closed.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
struct DiagnosticsState {
    invalid_json_samples: Mutex<VecDeque<String>>,
    invalid_json_sample_lines: usize,
    invalid_json_sample_max_bytes: usize,
}

impl DiagnosticsState {
    fn new(opts: &DiagnosticsOptions) -> Option<Arc<Self>> {
        if opts.invalid_json_sample_lines == 0 {
            return None;
        }
        Some(Arc::new(Self {
            invalid_json_samples: Mutex::new(VecDeque::with_capacity(
                opts.invalid_json_sample_lines,
            )),
            invalid_json_sample_lines: opts.invalid_json_sample_lines,
            invalid_json_sample_max_bytes: opts.invalid_json_sample_max_bytes.max(1),
        }))
    }

    fn record_invalid_json_line(&self, line: &[u8]) {
        let mut guard = self
            .invalid_json_samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if guard.len() >= self.invalid_json_sample_lines {
            guard.pop_front();
        }

        let sample_len = line.len().min(self.invalid_json_sample_max_bytes);
        let mut s = String::from_utf8_lossy(&line[..sample_len]).into_owned();
        if sample_len < line.len() {
            s.push('…');
        }
        s = truncate_string(s, self.invalid_json_sample_max_bytes);
        guard.push_back(s);
    }

    fn invalid_json_samples(&self) -> Vec<String> {
        self.invalid_json_samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

#[derive(Clone)]
pub struct ClientHandle {
    write: Arc<tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>>,
    next_id: Arc<AtomicI64>,
    pending: PendingRequests,
    max_pending_requests: usize,
    cancelled_request_ids: CancelledRequestIds,
    stats: Arc<ClientStatsInner>,
    diagnostics: Option<Arc<DiagnosticsState>>,
    closed: Arc<AtomicBool>,
    close_reason: Arc<OnceLock<String>>,
    stdout_log_write_error: Arc<OnceLock<String>>,
}

impl std::fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientHandle").finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct OutboundNotification<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
}

#[derive(Serialize)]
struct OutboundRequest<'a> {
    jsonrpc: &'static str,
    id: &'a Id,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
}

#[derive(Serialize)]
struct OutboundOkResponse<'a> {
    jsonrpc: &'static str,
    id: &'a Id,
    result: &'a Value,
}

#[derive(Serialize)]
struct OutboundErrorBody<'a> {
    code: i64,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Value>,
}

#[derive(Serialize)]
struct OutboundErrorResponse<'a, I> {
    jsonrpc: &'static str,
    id: &'a I,
    error: OutboundErrorBody<'a>,
}

fn serialize_json_line(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    Ok(line)
}

fn serialize_json_value(value: &impl Serialize) -> Result<Value, Error> {
    Ok(serde_json::to_value(value)?)
}

fn outbound_ok_response_value(id: &Id, result: &Value) -> Result<Value, Error> {
    serialize_json_value(&OutboundOkResponse {
        jsonrpc: "2.0",
        id,
        result,
    })
}

fn outbound_error_response_value<I>(
    id: &I,
    code: i64,
    message: &str,
    data: Option<&Value>,
) -> Result<Value, Error>
where
    I: Serialize,
{
    serialize_json_value(&OutboundErrorResponse {
        jsonrpc: "2.0",
        id,
        error: OutboundErrorBody {
            code,
            message,
            data,
        },
    })
}

struct BatchResponseState {
    handle: ClientHandle,
    responses: tokio::sync::Mutex<Vec<Value>>,
    pending_async_responses: AtomicU64,
    finished: AtomicBool,
    flushed: AtomicBool,
}

#[derive(Clone)]
struct BatchResponseWriter {
    state: Arc<BatchResponseState>,
}

struct DetachedRuntime {
    tx: std_mpsc::Sender<DetachedTask>,
}

impl DetachedRuntime {
    fn spawn(&self, task: DetachedTask) {
        let _ = self.tx.send(task);
    }
}

fn spawn_detached(task_name: &str, task: impl Future<Output = ()> + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(task));
        return;
    }

    detached_runtime(task_name).spawn(Box::pin(task));
}

fn detached_runtime(task_name: &str) -> &'static DetachedRuntime {
    static DETACHED_RUNTIME: OnceLock<DetachedRuntime> = OnceLock::new();
    DETACHED_RUNTIME.get_or_init(|| {
        let (tx, rx) = std_mpsc::channel::<DetachedTask>();
        std::thread::Builder::new()
            .name("mcp-jsonrpc-detached".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                while let Ok(task) = rx.recv() {
                    runtime.block_on(task);
                }
            })
            .unwrap_or_else(|err| {
                panic!("spawn detached mcp-jsonrpc runtime ({task_name}): {err}")
            });
        DetachedRuntime { tx }
    })
}

impl std::fmt::Debug for BatchResponseWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchResponseWriter")
            .finish_non_exhaustive()
    }
}

impl BatchResponseWriter {
    fn new(handle: ClientHandle) -> Self {
        Self {
            state: Arc::new(BatchResponseState {
                handle,
                responses: tokio::sync::Mutex::new(Vec::new()),
                pending_async_responses: AtomicU64::new(0),
                finished: AtomicBool::new(false),
                flushed: AtomicBool::new(false),
            }),
        }
    }

    fn reserve_request_slot(&self) -> Self {
        self.state
            .pending_async_responses
            .fetch_add(1, Ordering::Relaxed);
        self.clone()
    }

    async fn push_immediate_response(&self, response: Value) -> Result<(), Error> {
        self.state.handle.check_closed()?;
        self.state.responses.lock().await.push(response);
        Ok(())
    }

    async fn push_reserved_response(&self, response: Value) -> Result<(), Error> {
        self.state.handle.check_closed()?;
        self.state.responses.lock().await.push(response);
        self.state
            .pending_async_responses
            .fetch_sub(1, Ordering::Relaxed);
        self.flush_if_ready().await
    }

    fn push_reserved_response_without_runtime(&self, response: Value) {
        if self.state.handle.check_closed().is_err() {
            self.state
                .pending_async_responses
                .fetch_sub(1, Ordering::Relaxed);
            return;
        }

        self.state.responses.blocking_lock().push(response);
        self.state
            .pending_async_responses
            .fetch_sub(1, Ordering::Relaxed);
        self.flush_if_ready_without_runtime();
    }

    fn flush_if_ready_without_runtime(&self) {
        let batch = self.clone();
        spawn_detached("batch flush without runtime", async move {
            let _ = batch.flush_if_ready().await;
        });
    }

    async fn finish(&self) -> Result<(), Error> {
        self.state.finished.store(true, Ordering::Relaxed);
        self.flush_if_ready().await
    }

    async fn flush_if_ready(&self) -> Result<(), Error> {
        if !self.state.finished.load(Ordering::Relaxed)
            || self.state.pending_async_responses.load(Ordering::Relaxed) != 0
        {
            return Ok(());
        }

        if self
            .state
            .flushed
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Ok(());
        }

        let mut responses = self.state.responses.lock().await;
        if responses.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut *responses);
        drop(responses);
        self.state.handle.write_json_line(&batch).await
    }
}

#[derive(Clone)]
enum RequestResponseTarget {
    Direct(ClientHandle),
    Batch(BatchResponseWriter),
}

impl std::fmt::Debug for RequestResponseTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct(_) => f.debug_tuple("Direct").finish(),
            Self::Batch(_) => f.debug_tuple("Batch").finish(),
        }
    }
}

#[derive(Debug, Clone)]
struct RequestResponder {
    target: RequestResponseTarget,
    responded: Arc<AtomicBool>,
}

impl RequestResponder {
    fn direct(handle: ClientHandle) -> Self {
        Self {
            target: RequestResponseTarget::Direct(handle),
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    fn batch(batch: BatchResponseWriter) -> Self {
        Self {
            target: RequestResponseTarget::Batch(batch.reserve_request_slot()),
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn respond_ok(&self, id: &Id, result: Value) -> Result<(), Error> {
        let response = outbound_ok_response_value(id, &result)?;
        self.send_response(response).await
    }

    async fn respond_error(
        &self,
        id: &Id,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Result<(), Error> {
        let message = message.into();
        let response = outbound_error_response_value(id, code, &message, data.as_ref())?;
        self.send_response(response).await
    }

    async fn send_response(&self, response: Value) -> Result<(), Error> {
        if self
            .responded
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Err(Error::protocol(
                ProtocolErrorKind::Other,
                "request already responded",
            ));
        }

        match &self.target {
            RequestResponseTarget::Direct(handle) => handle.write_json_line(&response).await,
            RequestResponseTarget::Batch(batch) => batch.push_reserved_response(response).await,
        }
    }

    fn begin_drop_without_response(&self) -> bool {
        if Arc::strong_count(&self.responded) != 1 {
            return false;
        }

        self.responded
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

impl ClientHandle {
    pub fn stats(&self) -> ClientStats {
        self.stats.snapshot()
    }

    pub fn invalid_json_samples(&self) -> Vec<String> {
        self.diagnostics
            .as_ref()
            .map(|d| d.invalid_json_samples())
            .unwrap_or_default()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Returns the first recorded close diagnostic, if any.
    ///
    /// This is best-effort transport diagnostics, not a stable concurrency contract. When
    /// multiple close paths race, whichever path records first wins; later reasons are ignored.
    pub fn close_reason(&self) -> Option<String> {
        self.close_reason.get().cloned()
    }

    /// Returns the last stdout log write error, if any.
    ///
    /// When this is set, the client disables stdout log writes for the remainder of its
    /// lifetime. This is not treated as a fatal transport error.
    pub fn stdout_log_write_error(&self) -> Option<String> {
        self.stdout_log_write_error.get().cloned()
    }

    fn record_stdout_log_write_error(&self, err: &std::io::Error) {
        let _ = self.stdout_log_write_error.set(err.to_string());
    }

    pub async fn close(&self, reason: impl Into<String>) {
        self.close_with_reason(reason).await;
    }

    fn schedule_close_once(&self, reason: String) {
        if self
            .closed
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let _ = self.close_reason.set(reason.clone()); // pre-commit: allow-let-underscore
        let handle = self.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(async move {
                handle.close_with_reason(reason).await;
            }));
            return;
        }

        // No runtime available (e.g. sync context): avoid panicking and perform best-effort close.
        let err = Error::protocol(ProtocolErrorKind::Closed, reason);
        drain_pending(&handle.pending, &err);
        if let Ok(mut write) = handle.write.try_lock() {
            drop(std::mem::replace(&mut *write, Box::new(tokio::io::sink())));
        }
    }

    fn check_closed(&self) -> Result<(), Error> {
        if !self.closed.load(Ordering::Relaxed) {
            return Ok(());
        }
        let reason = self
            .close_reason
            .get()
            .cloned()
            .unwrap_or_else(|| "client closed".to_string());
        Err(Error::protocol(ProtocolErrorKind::Closed, reason))
    }

    pub(crate) async fn close_with_reason(&self, reason: impl Into<String>) {
        let reason = reason.into();
        self.close_with_error(
            reason.clone(),
            Error::protocol(ProtocolErrorKind::Closed, reason),
        )
        .await;
    }

    pub(crate) async fn close_with_error(&self, reason: impl Into<String>, err: Error) {
        let reason = reason.into();

        self.closed.store(true, Ordering::Relaxed);
        let _ = self.close_reason.set(reason);

        drain_pending(&self.pending, &err);
        let mut write = self.write.lock().await;
        let _ = write.shutdown().await;
        // Many `AsyncWrite` impls (e.g. `tokio::process::ChildStdin`) only fully close on drop.
        // Replacing the writer guarantees the underlying write end is closed.
        let _ = std::mem::replace(&mut *write, Box::new(tokio::io::sink()));
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), Error> {
        self.check_closed()?;
        let params = params.filter(|v| !v.is_null());
        let msg = OutboundNotification {
            jsonrpc: "2.0",
            method,
            params: params.as_ref(),
        };
        let line = serialize_json_line(&msg)?;
        self.write_line(&line).await?;
        Ok(())
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.request_optional(method, Some(params)).await
    }

    pub async fn request_optional(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, Error> {
        self.request_optional_inner(method, params, None).await
    }

    /// Send a JSON-RPC request and fail with `ProtocolErrorKind::WaitTimeout` if the response
    /// does not arrive before `timeout`.
    ///
    /// This requires a Tokio runtime with the time driver enabled.
    pub async fn request_optional_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, Error> {
        self.request_optional_inner(method, params, Some(timeout))
            .await
    }

    async fn request_optional_inner(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, Error> {
        if timeout.is_some() {
            ensure_tokio_time_driver("ClientHandle::request_optional_with_timeout")?;
        }
        self.check_closed()?;
        let id = Id::Integer(self.next_id.fetch_add(1, Ordering::Relaxed));

        let (tx, rx) = oneshot::channel::<Result<Value, Error>>();
        {
            let mut pending = lock_pending(&self.pending);
            if pending.len() >= self.max_pending_requests {
                return Err(Error::protocol(
                    ProtocolErrorKind::Other,
                    format!(
                        "too many pending requests (limit: {})",
                        self.max_pending_requests
                    ),
                ));
            }
            pending.insert(id.clone(), tx);
        }
        let mut guard = PendingRequestGuard::new(
            self.pending.clone(),
            self.cancelled_request_ids.clone(),
            id.clone(),
        );

        let params = params.filter(|v| !v.is_null());
        let req = OutboundRequest {
            jsonrpc: "2.0",
            id: &id,
            method,
            params: params.as_ref(),
        };
        let line = serialize_json_line(&req)?;
        let recv_result = match timeout {
            Some(timeout) => {
                let deadline = tokio::time::Instant::now() + timeout;
                match tokio::time::timeout_at(deadline, self.write_line(&line)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        lock_pending(&self.pending).remove(&id);
                        guard.disarm();
                        return Err(err);
                    }
                    Err(_) => {
                        let reason = format!("request timed out after {timeout:?} while writing");
                        self.schedule_close_once(reason);
                        return Err(Error::protocol(
                            ProtocolErrorKind::WaitTimeout,
                            format!("request timed out after {timeout:?}"),
                        ));
                    }
                }

                match tokio::time::timeout_at(deadline, rx).await {
                    Ok(result) => result,
                    Err(_) => {
                        return Err(Error::protocol(
                            ProtocolErrorKind::WaitTimeout,
                            format!("request timed out after {timeout:?}"),
                        ));
                    }
                }
            }
            None => {
                if let Err(err) = self.write_line(&line).await {
                    lock_pending(&self.pending).remove(&id);
                    guard.disarm();
                    return Err(err);
                }
                rx.await
            }
        };

        match recv_result {
            Ok(result) => {
                guard.disarm();
                result
            }
            Err(_) => Err(Error::protocol(
                ProtocolErrorKind::Closed,
                "response channel closed",
            )),
        }
    }

    pub async fn respond_ok(&self, id: Id, result: Value) -> Result<(), Error> {
        self.check_closed()?;
        self.write_json_line(&OutboundOkResponse {
            jsonrpc: "2.0",
            id: &id,
            result: &result,
        })
        .await
    }

    pub async fn respond_error(
        &self,
        id: Id,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Result<(), Error> {
        self.check_closed()?;
        let message = message.into();
        self.write_json_line(&OutboundErrorResponse {
            jsonrpc: "2.0",
            id: &id,
            error: OutboundErrorBody {
                code,
                message: &message,
                data: data.as_ref(),
            },
        })
        .await
    }

    pub(crate) async fn respond_error_raw_id(
        &self,
        id: Value,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Result<(), Error> {
        self.check_closed()?;
        let message = message.into();
        self.write_json_line(&OutboundErrorResponse {
            jsonrpc: "2.0",
            id: &id,
            error: OutboundErrorBody {
                code,
                message: &message,
                data: data.as_ref(),
            },
        })
        .await
    }

    async fn write_json_line(&self, value: &impl Serialize) -> Result<(), Error> {
        let line = serialize_json_line(value)?;
        self.write_line(&line).await
    }

    async fn write_line(&self, line: &[u8]) -> Result<(), Error> {
        self.check_closed()?;
        let mut write = self.write.lock().await;
        write.write_all(line).await?;
        write.flush().await?;
        drop(write);
        Ok(())
    }
}

pub struct Client {
    handle: ClientHandle,
    child: Option<Child>,
    notifications_rx: Option<mpsc::Receiver<Notification>>,
    requests_rx: Option<mpsc::Receiver<IncomingRequest>>,
    task: tokio::task::JoinHandle<()>,
    transport_tasks: Vec<tokio::task::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOnTimeout {
    /// Return an error if the child does not exit within the timeout.
    ///
    /// The child process is left running. Use `Client::take_child()` if you want to manage it
    /// manually.
    ReturnError,
    /// Attempt to kill the child if it does not exit within the timeout.
    ///
    /// After sending the kill signal, this waits up to `kill_timeout` for the child to exit.
    Kill { kill_timeout: Duration },
}

impl Client {
    pub fn stats(&self) -> ClientStats {
        self.handle.stats()
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn connect_io<R, W>(read: R, write: W) -> Result<Self, Error>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::connect_io_with_options(read, write, SpawnOptions::default()).await
    }

    pub async fn connect_io_with_options<R, W>(
        read: R,
        write: W,
        options: SpawnOptions,
    ) -> Result<Self, Error>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::create(read, write, None, options).await
    }

    pub async fn spawn<I, S>(program: S, args: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = OsString>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.stderr(Stdio::inherit());
        Self::spawn_command_with_options(cmd, SpawnOptions::default()).await
    }

    pub async fn spawn_with_options<I, S>(
        program: S,
        args: I,
        options: SpawnOptions,
    ) -> Result<Self, Error>
    where
        I: IntoIterator<Item = OsString>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.stderr(Stdio::inherit());
        Self::spawn_command_with_options(cmd, options).await
    }

    pub async fn spawn_command(cmd: Command) -> Result<Self, Error> {
        Self::spawn_command_with_options(cmd, SpawnOptions::default()).await
    }

    pub async fn spawn_command_with_options(
        mut cmd: Command,
        options: SpawnOptions,
    ) -> Result<Self, Error> {
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.kill_on_drop(options.kill_on_drop);

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::protocol(ProtocolErrorKind::Other, "child stdin not captured"))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::protocol(ProtocolErrorKind::Other, "child stdout not captured")
        })?;

        Self::create(stdout, stdin, Some(child), options).await
    }

    pub async fn connect_unix(path: &Path) -> Result<Self, Error> {
        #[cfg(unix)]
        {
            let stream = tokio::net::UnixStream::connect(path).await?;
            let (read, write) = stream.into_split();
            Self::create(read, write, None, SpawnOptions::default()).await
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(Error::protocol(
                ProtocolErrorKind::InvalidInput,
                "unix socket client is only supported on unix",
            ))
        }
    }

    async fn create<R, W>(
        read: R,
        write: W,
        child: Option<Child>,
        options: SpawnOptions,
    ) -> Result<Self, Error>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let SpawnOptions {
            stdout_log,
            stdout_log_redactor,
            limits,
            diagnostics,
            ..
        } = options;

        let notify_cap = limits.notifications_capacity.max(1);
        let request_cap = limits.requests_capacity.max(1);
        let max_pending_requests = limits.max_pending_requests.max(1);
        let (notify_tx, notify_rx) = mpsc::channel::<Notification>(notify_cap);
        let (request_tx, request_rx) = mpsc::channel::<IncomingRequest>(request_cap);
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids: CancelledRequestIds =
            Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let stats = Arc::new(ClientStatsInner::default());
        let write = Arc::new(tokio::sync::Mutex::new(Box::new(write) as _));
        let diagnostics_state = DiagnosticsState::new(&diagnostics);
        let handle = ClientHandle {
            write,
            next_id: Arc::new(AtomicI64::new(1)),
            pending: pending.clone(),
            max_pending_requests,
            cancelled_request_ids: cancelled_request_ids.clone(),
            stats: stats.clone(),
            diagnostics: diagnostics_state.clone(),
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(OnceLock::new()),
            stdout_log_write_error: Arc::new(OnceLock::new()),
        };

        let stdout_log = match stdout_log {
            Some(opts) => Some(LogState::new(opts).await?),
            None => None,
        };
        let task = spawn_reader_task(
            read,
            ReaderTaskContext {
                pending,
                cancelled_request_ids,
                stats,
                notify_tx,
                request_tx,
                responder: handle.clone(),
                stdout_log,
                stdout_log_redactor,
                diagnostics_state,
                limits,
            },
        );

        Ok(Self {
            handle,
            child,
            notifications_rx: Some(notify_rx),
            requests_rx: Some(request_rx),
            task,
            transport_tasks: Vec::new(),
        })
    }

    pub fn handle(&self) -> ClientHandle {
        self.handle.clone()
    }

    pub async fn close(&self, reason: impl Into<String>) {
        self.task.abort();
        for task in &self.transport_tasks {
            task.abort();
        }
        self.handle.close(reason).await;
    }

    /// Schedule an asynchronous close only once.
    ///
    /// This marks the client closed immediately and starts a best-effort background close path.
    /// Repeated calls after the first one are no-ops.
    pub fn close_in_background_once(&self, reason: impl Into<String>) {
        self.handle.schedule_close_once(reason.into());
    }

    pub fn child_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(tokio::process::Child::id)
    }

    pub fn take_child(&mut self) -> Option<Child> {
        self.child.take()
    }

    pub fn take_notifications(&mut self) -> Option<mpsc::Receiver<Notification>> {
        self.notifications_rx.take()
    }

    pub fn take_requests(&mut self) -> Option<mpsc::Receiver<IncomingRequest>> {
        self.requests_rx.take()
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), Error> {
        self.handle.notify(method, params).await
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.handle.request(method, params).await
    }

    pub async fn request_optional(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, Error> {
        self.handle.request_optional(method, params).await
    }

    /// Send a JSON-RPC request and fail with `ProtocolErrorKind::WaitTimeout` if the response
    /// does not arrive before `timeout`.
    ///
    /// This requires a Tokio runtime with the time driver enabled.
    pub async fn request_optional_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, Error> {
        self.handle
            .request_optional_with_timeout(method, params, timeout)
            .await
    }

    /// Closes the client and (if present) waits for the underlying child process to exit.
    ///
    /// Clients created without a child process (e.g. via `connect_io`, `connect_unix`, or
    /// `connect_streamable_http*`) return `Ok(None)`.
    ///
    /// Note: this method can hang indefinitely if the child process does not exit.
    /// Prefer `Client::wait_with_timeout` if you need an upper bound.
    pub async fn wait(&mut self) -> Result<Option<std::process::ExitStatus>, Error> {
        self.task.abort();
        for task in self.transport_tasks.drain(..) {
            task.abort();
        }
        self.handle.close_with_reason("client closed").await;

        match &mut self.child {
            Some(child) => Ok(Some(child.wait().await?)),
            None => Ok(None),
        }
    }

    /// Closes the client and waits for the underlying child process to exit, up to `timeout`.
    ///
    /// If this client has no child process (e.g. created via `connect_io`, `connect_unix`, or
    /// `connect_streamable_http*`), this returns `Ok(None)` without waiting.
    ///
    /// On timeout:
    /// - `WaitOnTimeout::ReturnError` returns an `Error::Protocol` with kind
    ///   `ProtocolErrorKind::WaitTimeout` and leaves the child running.
    /// - `WaitOnTimeout::Kill { kill_timeout }` sends a kill signal, then waits up to
    ///   `kill_timeout` for the child to exit.
    ///
    /// This requires a Tokio runtime with the time driver enabled.
    pub async fn wait_with_timeout(
        &mut self,
        timeout: Duration,
        on_timeout: WaitOnTimeout,
    ) -> Result<Option<std::process::ExitStatus>, Error> {
        ensure_tokio_time_driver("Client::wait_with_timeout")?;
        let deadline = tokio::time::Instant::now() + timeout;
        self.task.abort();
        for task in self.transport_tasks.drain(..) {
            task.abort();
        }
        if tokio::time::timeout_at(deadline, self.handle.close_with_reason("client closed"))
            .await
            .is_err()
        {
            if let WaitOnTimeout::Kill { kill_timeout } = on_timeout {
                if let Some(child) = &mut self.child {
                    let child_id = child.id();
                    if let Err(err) = child.start_kill() {
                        return match child.try_wait() {
                            Ok(Some(status)) => Ok(Some(status)),
                            Ok(None) => Err(Error::protocol(
                                ProtocolErrorKind::WaitTimeout,
                                format!(
                                    "wait timed out after {timeout:?} while closing client; failed to kill child (id={child_id:?}): {err}"
                                ),
                            )),
                            Err(try_wait_err) => Err(Error::protocol(
                                ProtocolErrorKind::WaitTimeout,
                                format!(
                                    "wait timed out after {timeout:?} while closing client; failed to kill child (id={child_id:?}): {err}; try_wait failed: {try_wait_err}"
                                ),
                            )),
                        };
                    }
                    return match tokio::time::timeout(kill_timeout, child.wait()).await {
                        Ok(status) => Ok(Some(status?)),
                        Err(_) => Err(Error::protocol(
                            ProtocolErrorKind::WaitTimeout,
                            format!(
                                "wait timed out after {timeout:?} while closing client; killed child (id={child_id:?}) but it did not exit within {kill_timeout:?}"
                            ),
                        )),
                    };
                }
            }

            return Err(Error::protocol(
                ProtocolErrorKind::WaitTimeout,
                format!("wait timed out after {timeout:?} while closing client"),
            ));
        }

        let Some(child) = &mut self.child else {
            return Ok(None);
        };
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());

        match tokio::time::timeout(remaining, child.wait()).await {
            Ok(status) => Ok(Some(status?)),
            Err(_) => match on_timeout {
                WaitOnTimeout::ReturnError => Err(Error::protocol(
                    ProtocolErrorKind::WaitTimeout,
                    format!("wait timed out after {timeout:?}"),
                )),
                WaitOnTimeout::Kill { kill_timeout } => {
                    let child_id = child.id();
                    if let Err(err) = child.start_kill() {
                        match child.try_wait() {
                            Ok(Some(status)) => return Ok(Some(status)),
                            Ok(None) => {
                                return Err(Error::protocol(
                                    ProtocolErrorKind::WaitTimeout,
                                    format!(
                                        "wait timed out after {timeout:?}; failed to kill child (id={child_id:?}): {err}"
                                    ),
                                ));
                            }
                            Err(try_wait_err) => {
                                return Err(Error::protocol(
                                    ProtocolErrorKind::WaitTimeout,
                                    format!(
                                        "wait timed out after {timeout:?}; failed to kill child (id={child_id:?}): {err}; try_wait failed: {try_wait_err}"
                                    ),
                                ));
                            }
                        }
                    }

                    match tokio::time::timeout(kill_timeout, child.wait()).await {
                        Ok(status) => Ok(Some(status?)),
                        Err(_) => Err(Error::protocol(
                            ProtocolErrorKind::WaitTimeout,
                            format!(
                                "wait timed out after {timeout:?}; killed child (id={child_id:?}) but it did not exit within {kill_timeout:?}"
                            ),
                        )),
                    }
                }
            },
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.handle.closed.store(true, Ordering::Relaxed);
        let _ = self.handle.close_reason.set("client closed".to_string());
        self.task.abort();
        for task in self.transport_tasks.drain(..) {
            task.abort();
        }
        // Best-effort: eagerly drop the underlying writer even if cloned handles remain.
        if let Ok(mut write) = self.handle.write.try_lock() {
            drop(std::mem::replace(&mut *write, Box::new(tokio::io::sink())));
        }
        let err = Error::protocol(ProtocolErrorKind::Closed, "client closed");
        drain_pending(&self.handle.pending, &err);
    }
}

struct PendingRequestGuard {
    pending: PendingRequests,
    cancelled_request_ids: CancelledRequestIds,
    id: Id,
    armed: bool,
}

impl PendingRequestGuard {
    fn new(pending: PendingRequests, cancelled_request_ids: CancelledRequestIds, id: Id) -> Self {
        Self {
            pending,
            cancelled_request_ids,
            id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut pending = lock_pending(&self.pending);
        pending.remove(&self.id);
        drop(pending);
        remember_cancelled_request_id(&self.cancelled_request_ids, &self.id);
    }
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct IncomingRequest {
    pub id: Id,
    pub method: String,
    pub params: Option<Value>,
    responder: RequestResponder,
}

impl IncomingRequest {
    pub async fn respond_ok(&self, result: Value) -> Result<(), Error> {
        self.responder.respond_ok(&self.id, result).await
    }

    pub async fn respond_error(
        &self,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Result<(), Error> {
        self.responder
            .respond_error(&self.id, code, message, data)
            .await
    }
}

impl Drop for IncomingRequest {
    fn drop(&mut self) {
        const INTERNAL_ERROR: i64 = -32603;
        const DROPPED_REQUEST_MESSAGE: &str = "request handler dropped request without responding";

        if !self.responder.begin_drop_without_response() {
            return;
        }

        let response = match outbound_error_response_value(
            &self.id,
            INTERNAL_ERROR,
            DROPPED_REQUEST_MESSAGE,
            None,
        ) {
            Ok(response) => response,
            Err(_) => return,
        };

        match &self.responder.target {
            RequestResponseTarget::Direct(handle) => {
                let handle = handle.clone();
                spawn_detached("direct dropped request response", async move {
                    drop(handle.write_json_line(&response).await);
                });
            }
            RequestResponseTarget::Batch(batch) => {
                if tokio::runtime::Handle::try_current().is_ok() {
                    let batch = batch.clone();
                    spawn_detached("batch dropped request response", async move {
                        drop(batch.push_reserved_response(response).await);
                    });
                } else {
                    batch.push_reserved_response_without_runtime(response);
                }
            }
        }
    }
}

struct ReaderTaskContext {
    pending: PendingRequests,
    cancelled_request_ids: CancelledRequestIds,
    stats: Arc<ClientStatsInner>,
    notify_tx: mpsc::Sender<Notification>,
    request_tx: mpsc::Sender<IncomingRequest>,
    responder: ClientHandle,
    stdout_log: Option<LogState>,
    stdout_log_redactor: Option<StdoutLogRedactor>,
    diagnostics_state: Option<Arc<DiagnosticsState>>,
    limits: Limits,
}

fn spawn_reader_task<R>(reader: R, ctx: ReaderTaskContext) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let ReaderTaskContext {
            pending,
            cancelled_request_ids,
            stats,
            notify_tx,
            request_tx,
            responder,
            stdout_log,
            stdout_log_redactor,
            diagnostics_state,
            limits,
        } = ctx;

        let mut log_state = stdout_log;

        let max_message_bytes = normalize_max_message_bytes(limits.max_message_bytes);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            let next = read_line_limited_into(&mut reader, max_message_bytes, &mut line).await;
            match next {
                Ok(true) => {
                    if is_ascii_whitespace_only(&line) {
                        continue;
                    }
                    if let Some(state) = &mut log_state {
                        let write_result = match &stdout_log_redactor {
                            Some(redactor) => state.write_line_bytes(&redactor(&line)).await,
                            None => state.write_line_bytes(&line).await,
                        };
                        if let Err(err) = write_result {
                            responder.record_stdout_log_write_error(&err);
                            log_state = None;
                        }
                    }
                    let value: Value = match serde_json::from_slice(&line) {
                        Ok(value) => value,
                        Err(_) => {
                            stats.invalid_json_lines.fetch_add(1, Ordering::Relaxed);
                            if let Some(diagnostics) = &diagnostics_state {
                                diagnostics.record_invalid_json_line(&line);
                            }
                            close_invalid_message(
                                &responder,
                                "peer sent invalid JSON line".to_string(),
                            )
                            .await;
                            return;
                        }
                    };
                    if let Err(reason) = handle_incoming_value(
                        value,
                        &pending,
                        &cancelled_request_ids,
                        &stats,
                        &notify_tx,
                        &request_tx,
                        &responder,
                    )
                    .await
                    {
                        close_invalid_message(&responder, reason).await;
                        return;
                    }
                    if responder.is_closed() {
                        return;
                    }
                }
                Ok(false) => {
                    responder
                        .close_with_reason("server closed connection")
                        .await;
                    return;
                }
                Err(err) => {
                    let reason = format!("io error: {err}");
                    responder.close_with_error(reason, Error::Io(err)).await;
                    return;
                }
            }
        }
    })
}

async fn close_invalid_message(responder: &ClientHandle, reason: String) {
    responder
        .close_with_error(
            reason.clone(),
            Error::protocol(ProtocolErrorKind::InvalidMessage, reason),
        )
        .await;
}

async fn handle_incoming_value(
    value: Value,
    pending: &PendingRequests,
    cancelled_request_ids: &CancelledRequestIds,
    stats: &Arc<ClientStatsInner>,
    notify_tx: &mpsc::Sender<Notification>,
    request_tx: &mpsc::Sender<IncomingRequest>,
    responder: &ClientHandle,
) -> Result<(), String> {
    const INVALID_REQUEST: i64 = -32600;
    let ctx = IncomingValueContext {
        pending,
        cancelled_request_ids,
        stats,
        notify_tx,
        request_tx,
        responder,
    };

    match value {
        Value::Array(items) => {
            if items.is_empty() {
                let _ = ctx
                    .responder
                    .respond_error_raw_id(Value::Null, INVALID_REQUEST, "empty batch", None)
                    .await;
                return Err("peer sent empty JSON-RPC batch".to_string());
            }

            let batch = BatchResponseWriter::new(ctx.responder.clone());
            let mut stack = Vec::with_capacity(items.len());
            stack.extend(items.into_iter().rev().map(|item| (item, false)));
            while let Some((item, allow_batch_expansion)) = stack.pop() {
                if let Err(reason) = handle_incoming_item(
                    item,
                    allow_batch_expansion,
                    &ctx,
                    Some(&batch),
                    &mut stack,
                )
                .await
                {
                    let _ = batch.finish().await;
                    return Err(reason);
                }
                if ctx.responder.is_closed() {
                    return Ok(());
                }
            }
            let _ = batch.finish().await;
            Ok(())
        }
        other => {
            let mut stack = Vec::new();
            handle_incoming_item(other, true, &ctx, None, &mut stack).await
        }
    }
}

async fn close_notification_queue_overflow(
    ctx: &IncomingValueContext<'_>,
    reason: String,
    counter: &AtomicU64,
) {
    counter.fetch_add(1, Ordering::Relaxed);
    ctx.responder
        .close_with_error(
            reason.clone(),
            Error::protocol(ProtocolErrorKind::Other, reason),
        )
        .await;
}

struct IncomingValueContext<'a> {
    pending: &'a PendingRequests,
    cancelled_request_ids: &'a CancelledRequestIds,
    stats: &'a Arc<ClientStatsInner>,
    notify_tx: &'a mpsc::Sender<Notification>,
    request_tx: &'a mpsc::Sender<IncomingRequest>,
    responder: &'a ClientHandle,
}

async fn handle_incoming_item(
    value: Value,
    allow_batch_expansion: bool,
    ctx: &IncomingValueContext<'_>,
    batch: Option<&BatchResponseWriter>,
    stack: &mut Vec<(Value, bool)>,
) -> Result<(), String> {
    const INVALID_REQUEST: i64 = -32600;

    match value {
        Value::Array(items) => {
            if !allow_batch_expansion {
                let _ = send_batch_or_direct_error_raw_id(
                    ctx.responder,
                    batch,
                    Value::Null,
                    INVALID_REQUEST,
                    "nested batch is not allowed",
                    None,
                )
                .await;
                return Err("peer sent nested JSON-RPC batch".to_string());
            }

            if items.is_empty() {
                let _ = send_batch_or_direct_error_raw_id(
                    ctx.responder,
                    batch,
                    Value::Null,
                    INVALID_REQUEST,
                    "empty batch",
                    None,
                )
                .await;
                return Err("peer sent empty nested JSON-RPC batch".to_string());
            }

            stack.reserve(items.len());
            stack.extend(items.into_iter().rev().map(|item| (item, false)));
            Ok(())
        }
        Value::Object(mut map) => {
            let jsonrpc_valid = map.get("jsonrpc").and_then(Value::as_str) == Some("2.0");

            match map.remove("method") {
                Some(Value::String(method)) => {
                    let id_value = map.remove("id");
                    if !jsonrpc_valid {
                        if let Some(id_value) = id_value {
                            let id_value = error_response_id_or_null(id_value);
                            let _ = send_batch_or_direct_error_raw_id(
                                ctx.responder,
                                batch,
                                id_value,
                                INVALID_REQUEST,
                                "invalid jsonrpc version",
                                None,
                            )
                            .await;
                        }
                        return Err(
                            "peer sent request/notification with invalid jsonrpc version"
                                .to_string(),
                        );
                    }

                    let params = map.remove("params");
                    if let Some(id_value) = id_value {
                        let Some(id) = parse_id_owned(id_value) else {
                            let _ = send_batch_or_direct_error_raw_id(
                                ctx.responder,
                                batch,
                                Value::Null,
                                INVALID_REQUEST,
                                "invalid request id",
                                None,
                            )
                            .await;
                            return Err("peer sent request with invalid id".to_string());
                        };

                        let request = IncomingRequest {
                            id,
                            method,
                            params,
                            responder: match batch {
                                Some(batch) => RequestResponder::batch(batch.clone()),
                                None => RequestResponder::direct(ctx.responder.clone()),
                            },
                        };

                        match ctx.request_tx.try_send(request) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(request)) => {
                                drop(
                                    request
                                        .responder
                                        .respond_error(
                                            &request.id,
                                            -32000,
                                            "client overloaded",
                                            None,
                                        )
                                        .await,
                                );
                            }
                            Err(mpsc::error::TrySendError::Closed(request)) => {
                                drop(
                                    request
                                        .responder
                                        .respond_error(
                                            &request.id,
                                            -32601,
                                            "no request handler installed",
                                            None,
                                        )
                                        .await,
                                );
                            }
                        }
                        return Ok(());
                    }

                    match ctx.notify_tx.try_send(Notification { method, params }) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            close_notification_queue_overflow(
                                ctx,
                                "server notification queue is full; closing connection to avoid silent data loss".to_string(),
                                &ctx.stats.dropped_notifications_full,
                            )
                            .await;
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            close_notification_queue_overflow(
                                ctx,
                                "server notification handler is unavailable; closing connection to avoid silent data loss".to_string(),
                                &ctx.stats.dropped_notifications_closed,
                            )
                            .await;
                        }
                    }
                    return Ok(());
                }
                Some(_) => {
                    if let Some(id_value) = map.remove("id") {
                        let id_value = error_response_id_or_null(id_value);
                        let _ = send_batch_or_direct_error_raw_id(
                            ctx.responder,
                            batch,
                            id_value,
                            INVALID_REQUEST,
                            "invalid request method",
                            None,
                        )
                        .await;
                    }
                    return Err("peer sent request with invalid method".to_string());
                }
                None => {}
            }

            handle_response(ctx.pending, ctx.cancelled_request_ids, Value::Object(map))
                .map_err(|err| err.to_string())
        }
        _ => {
            let _ = send_batch_or_direct_error_raw_id(
                ctx.responder,
                batch,
                Value::Null,
                INVALID_REQUEST,
                "invalid message",
                None,
            )
            .await;
            Err("peer sent non-object JSON-RPC message".to_string())
        }
    }
}

async fn send_batch_or_direct_error_raw_id(
    responder: &ClientHandle,
    batch: Option<&BatchResponseWriter>,
    id: Value,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> Result<(), Error> {
    let message = message.into();
    match batch {
        Some(batch) => {
            batch
                .push_immediate_response(outbound_error_response_value(
                    &id,
                    code,
                    &message,
                    data.as_ref(),
                )?)
                .await
        }
        None => {
            responder
                .respond_error_raw_id(id, code, message, data)
                .await
        }
    }
}

async fn read_line_limited<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut buf = Vec::with_capacity(max_bytes.min(READ_LINE_INITIAL_CAP_BYTES));
    if read_line_limited_into(reader, max_bytes, &mut buf).await? {
        Ok(Some(buf))
    } else {
        Ok(None)
    }
}

async fn read_line_limited_into<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
    buf: &mut Vec<u8>,
) -> Result<bool, std::io::Error> {
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.len() > max_bytes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "jsonrpc message too large",
                ));
            }
            maybe_shrink_line_buffer(buf, max_bytes);
            return Ok(!buf.is_empty());
        }

        let newline_pos = available.iter().position(|b| *b == b'\n');
        let take = newline_pos.map_or(available.len(), |idx| idx.saturating_add(1));
        // Allow only delimiter slack above the payload limit:
        // - up to 1 byte while scanning (possible trailing '\r' before '\n')
        // - up to 2 bytes when this chunk includes '\n' (possible "\r\n")
        let delimiter_slack = if newline_pos.is_some() { 2 } else { 1 };
        if buf.len().saturating_add(take) > max_bytes.saturating_add(delimiter_slack) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "jsonrpc message too large",
            ));
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);

        if newline_pos.is_some() {
            break;
        }
    }

    if buf.ends_with(b"\n") {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
    }

    if buf.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "jsonrpc message too large",
        ));
    }

    maybe_shrink_line_buffer(buf, max_bytes);
    Ok(true)
}

pub(crate) fn is_ascii_whitespace_only(line: &[u8]) -> bool {
    line.is_empty()
        || (line.first().is_some_and(u8::is_ascii_whitespace)
            && line.iter().all(u8::is_ascii_whitespace))
}

fn maybe_shrink_line_buffer(buf: &mut Vec<u8>, max_bytes: usize) {
    let retain = REUSABLE_LINE_BUFFER_RETAIN_BYTES.min(max_bytes);
    if retain == 0 {
        return;
    }
    // After occasional large messages, release surplus capacity once smaller traffic resumes.
    if buf.capacity() > retain && buf.len() <= retain {
        buf.shrink_to(retain);
    }
}

fn truncate_string(mut s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    s.truncate(end);
    s
}

fn lock_pending(
    pending: &PendingRequests,
) -> std::sync::MutexGuard<'_, HashMap<Id, oneshot::Sender<Result<Value, Error>>>> {
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_cancelled_request_ids(
    cancelled_request_ids: &CancelledRequestIds,
) -> std::sync::MutexGuard<'_, CancelledRequestIdsState> {
    cancelled_request_ids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remember_cancelled_request_id(cancelled_request_ids: &CancelledRequestIds, id: &Id) {
    let mut guard = lock_cancelled_request_ids(cancelled_request_ids);
    while guard.order.len() >= CANCELLED_REQUEST_IDS_MAX {
        let Some((generation, evicted)) = guard.order.pop_front() else {
            break;
        };
        if guard.latest.get(&evicted).copied() == Some(generation) {
            guard.latest.remove(&evicted);
        }
    }
    let generation = guard.next_generation;
    guard.next_generation = guard.next_generation.wrapping_add(1);
    guard.order.push_back((generation, id.clone()));
    guard.latest.insert(id.clone(), generation);
}

fn take_cancelled_request_id(cancelled_request_ids: &CancelledRequestIds, id: &Id) -> bool {
    let mut guard = lock_cancelled_request_ids(cancelled_request_ids);
    guard.latest.remove(id).is_some()
}

fn type_mismatch_candidate_id(id: &Id) -> Option<Id> {
    match id {
        Id::Integer(value) => Some(Id::String(value.to_string())),
        Id::Unsigned(value) => Some(Id::String(value.to_string())),
        Id::String(value) => parse_stringified_numeric_id(value),
    }
}

fn parse_stringified_numeric_id(value: &str) -> Option<Id> {
    match value.parse::<i64>() {
        Ok(parsed) if parsed.to_string() == value => return Some(Id::Integer(parsed)),
        _ => {}
    }

    match value.parse::<u64>() {
        Ok(parsed) if parsed.to_string() == value => Some(Id::Unsigned(parsed)),
        _ => None,
    }
}

fn take_cancelled_request_id_type_mismatch(
    cancelled_request_ids: &CancelledRequestIds,
    id: &Id,
) -> bool {
    let Some(candidate) = type_mismatch_candidate_id(id) else {
        return false;
    };

    let mut guard = lock_cancelled_request_ids(cancelled_request_ids);
    guard.latest.remove(&candidate).is_some()
}

fn take_pending_type_mismatch_sender(
    pending: &PendingRequests,
    id: &Id,
) -> Option<oneshot::Sender<Result<Value, Error>>> {
    let candidate = type_mismatch_candidate_id(id)?;

    let mut pending = lock_pending(pending);
    pending.remove(&candidate)
}

fn drain_pending(pending: &PendingRequests, err: &Error) {
    let pending = {
        let mut pending = lock_pending(pending);
        std::mem::take(&mut *pending)
    };

    for (_id, tx) in pending {
        let _ = tx.send(Err(clone_error_for_drain(err)));
    }
}

fn clone_error_for_drain(err: &Error) -> Error {
    match err {
        Error::Io(err) => Error::Io(std::io::Error::new(err.kind(), err.to_string())),
        Error::Json(err) => Error::protocol(ProtocolErrorKind::Other, format!("json error: {err}")),
        Error::Rpc {
            code,
            message,
            data,
        } => Error::Rpc {
            code: *code,
            message: message.clone(),
            data: data.clone(),
        },
        Error::Protocol(err) => Error::Protocol(err.clone()),
    }
}

fn error_response_id_or_null(value: Value) -> Value {
    match value {
        Value::String(_) | Value::Number(_) => value,
        _ => Value::Null,
    }
}

fn parse_id_owned(value: Value) -> Option<Id> {
    match value {
        Value::String(value) => Some(Id::String(value)),
        Value::Number(value) => value
            .as_i64()
            .map(Id::Integer)
            .or_else(|| value.as_u64().map(Id::Unsigned)),
        _ => None,
    }
}

fn handle_response(
    pending: &PendingRequests,
    cancelled_request_ids: &CancelledRequestIds,
    value: Value,
) -> Result<(), Error> {
    let Value::Object(mut map) = value else {
        return Err(Error::protocol(
            ProtocolErrorKind::InvalidMessage,
            "invalid response: not an object",
        ));
    };

    let Some(id_value) = map.remove("id") else {
        return Err(Error::protocol(
            ProtocolErrorKind::InvalidMessage,
            "invalid response: missing id",
        ));
    };
    let Some(id) = parse_id_owned(id_value) else {
        return Err(Error::protocol(
            ProtocolErrorKind::InvalidMessage,
            "invalid response: invalid id",
        ));
    };

    let tx = {
        let mut pending = lock_pending(pending);
        pending.remove(&id)
    };
    let Some(tx) = tx else {
        if take_cancelled_request_id(cancelled_request_ids, &id)
            || take_cancelled_request_id_type_mismatch(cancelled_request_ids, &id)
        {
            return Ok(());
        }
        if let Some(tx) = take_pending_type_mismatch_sender(pending, &id) {
            let _ = /* pre-commit: allow-let-underscore */ tx.send(Err(Error::protocol(
                ProtocolErrorKind::InvalidMessage,
                "invalid response: response id type mismatch",
            )));
            return Ok(());
        }
        // Unknown response ids are treated as stale/stray and ignored to avoid tearing down
        // otherwise healthy connections when late responses arrive after local timeout/cancel.
        return Ok(());
    };

    if map.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        let _ = tx.send(Err(Error::protocol(
            ProtocolErrorKind::InvalidMessage,
            "invalid response jsonrpc version",
        )));
        return Ok(());
    }

    let error = map.remove("error");
    let result = map.remove("result");
    match (error, result) {
        (Some(Value::Object(mut error)), None) => {
            let Some(code) = error.remove("code").and_then(|v| v.as_i64()) else {
                let _ = tx.send(Err(Error::protocol(
                    ProtocolErrorKind::InvalidMessage,
                    "invalid error response",
                )));
                return Ok(());
            };
            let Some(message) = error.remove("message").and_then(|v| match v {
                Value::String(message) => Some(message),
                _ => None,
            }) else {
                let _ = tx.send(Err(Error::protocol(
                    ProtocolErrorKind::InvalidMessage,
                    "invalid error response",
                )));
                return Ok(());
            };
            let data = error.remove("data");
            let _ = tx.send(Err(Error::Rpc {
                code,
                message,
                data,
            }));
            Ok(())
        }
        (None, Some(result)) => {
            let _ = tx.send(Ok(result));
            Ok(())
        }
        _ => {
            let _ = tx.send(Err(Error::protocol(
                ProtocolErrorKind::InvalidMessage,
                "invalid response: must include exactly one of result/error",
            )));
            Ok(())
        }
    }
}

// Streamable HTTP and stdout_log implementations live in `streamable_http.rs` and
// `stdout_log.rs`.

#[cfg(test)]
mod line_limit_tests {
    use super::*;

    #[tokio::test]
    async fn read_line_limited_accepts_payload_equal_to_limit_with_lf() {
        let input = b"hello\n";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let line = match read_line_limited(&mut reader, 5).await {
            Ok(Some(line)) => line,
            Ok(None) => panic!("line available"),
            Err(err) => panic!("read succeeds: {err}"),
        };

        assert_eq!(line, b"hello");
    }

    #[tokio::test]
    async fn read_line_limited_accepts_payload_equal_to_limit_with_crlf() {
        let input = b"hello\r\n";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let line = match read_line_limited(&mut reader, 5).await {
            Ok(Some(line)) => line,
            Ok(None) => panic!("line available"),
            Err(err) => panic!("read succeeds: {err}"),
        };

        assert_eq!(line, b"hello");
    }

    #[tokio::test]
    async fn read_line_limited_rejects_payload_over_limit() {
        let input = b"helloo\n";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let err = read_line_limited(&mut reader, 5)
            .await
            .expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_line_limited_rejects_payload_over_limit_without_newline() {
        let input = b"helloo";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let err = read_line_limited(&mut reader, 5)
            .await
            .expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_line_limited_into_releases_large_buffer_after_small_line() {
        let large = vec![b'x'; REUSABLE_LINE_BUFFER_RETAIN_BYTES * 2];
        let mut input = large.clone();
        input.push(b'\n');
        input.extend_from_slice(b"ok\n");

        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input));
        let mut line = Vec::new();

        assert!(
            read_line_limited_into(&mut reader, large.len(), &mut line)
                .await
                .expect("large line must parse")
        );
        let large_capacity = line.capacity();
        assert!(large_capacity >= large.len());

        assert!(
            read_line_limited_into(&mut reader, large.len(), &mut line)
                .await
                .expect("small line must parse")
        );
        assert_eq!(line, b"ok");
        assert!(line.capacity() <= REUSABLE_LINE_BUFFER_RETAIN_BYTES);
    }

    #[test]
    fn ascii_whitespace_only_fast_path_keeps_semantics() {
        assert!(is_ascii_whitespace_only(b""));
        assert!(is_ascii_whitespace_only(b" \t\r\n"));
        assert!(!is_ascii_whitespace_only(b"{\"jsonrpc\":\"2.0\"}"));
        assert!(!is_ascii_whitespace_only(b"\xE3\x80\x80"));
    }
}

#[cfg(test)]
mod incoming_value_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn nested_batch_item_returns_invalid_request_error_in_batch_array() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(br#"[[{"jsonrpc":"2.0","method":"demo"}]]"#)
            .await
            .expect("write nested batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush nested batch");

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["error"]["code"], -32600);
        assert_eq!(items[0]["error"]["message"], "nested batch is not allowed");
        assert_eq!(items[0]["id"], Value::Null);

        drop(client);
    }

    #[tokio::test]
    async fn top_level_batch_request_returns_single_array_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(
                br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","method":"note"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
            )
            .await
            .expect("write batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush batch");

        let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("first request timeout")
            .expect("first request");
        assert_eq!(first.method, "first");

        let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("second request timeout")
            .expect("second request");
        assert_eq!(second.method, "second");

        first
            .respond_ok(serde_json::json!({"handled":"first"}))
            .await
            .expect("respond first");
        second
            .respond_error(-32001, "second failed", None)
            .await
            .expect("respond second");

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 2);
        assert_eq!(items.iter().filter(|item| item["id"] == 1).count(), 1);
        assert_eq!(items.iter().filter(|item| item["id"] == 2).count(), 1);
        assert!(
            items
                .iter()
                .any(|item| item["id"] == 1 && item["result"]["handled"] == "first"),
            "{items:?}"
        );
        assert!(
            items.iter().any(|item| {
                item["id"] == 2
                    && item["error"]["code"] == -32001
                    && item["error"]["message"] == "second failed"
            }),
            "{items:?}"
        );

        drop(client);
    }

    #[tokio::test]
    async fn dropped_direct_request_returns_internal_error_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(br#"{"jsonrpc":"2.0","id":1,"method":"first"}"#)
            .await
            .expect("write request");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush request");

        let request = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("request timeout")
            .expect("request");
        assert_eq!(request.method, "first");
        drop(request);

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse response");
        assert_eq!(response["id"], 1);
        assert_eq!(response["error"]["code"], -32603);
        assert_eq!(
            response["error"]["message"],
            "request handler dropped request without responding"
        );

        drop(client);
    }

    #[tokio::test]
    async fn dropped_batch_request_emits_internal_error_and_preserves_remaining_batch_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(
                br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
            )
            .await
            .expect("write batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush batch");

        let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("first request timeout")
            .expect("first request");
        assert_eq!(first.method, "first");

        let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("second request timeout")
            .expect("second request");
        assert_eq!(second.method, "second");

        second
            .respond_ok(serde_json::json!({"handled":"second"}))
            .await
            .expect("respond second");
        drop(first);

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 2);
        assert!(
            items.iter().any(|item| {
                item["id"] == 1
                    && item["error"]["code"] == -32603
                    && item["error"]["message"]
                        == "request handler dropped request without responding"
            }),
            "{items:?}"
        );
        assert!(
            items
                .iter()
                .any(|item| item["id"] == 2 && item["result"]["handled"] == "second"),
            "{items:?}"
        );

        drop(client);
    }

    #[test]
    fn dropped_batch_request_without_runtime_still_flushes_remaining_batch_response() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, server_read, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");
            let mut requests = client.take_requests().expect("request receiver");

            server_write
                .write_all(
                    br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
                )
                .await
                .expect("write batch");
            server_write.write_all(b"\n").await.expect("write newline");
            server_write.flush().await.expect("flush batch");

            let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
                .await
                .expect("first request timeout")
                .expect("first request");
            let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
                .await
                .expect("second request timeout")
                .expect("second request");

            second
                .respond_ok(serde_json::json!({"handled":"second"}))
                .await
                .expect("respond second");

            (first, server_read, client)
        });
        drop(runtime);

        drop(request);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build read runtime");

        runtime.block_on(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();
            let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .expect("response timeout")
                .expect("read response")
                .expect("response line");
            let response: Value = serde_json::from_str(&response_line).expect("parse batch");
            let items = response.as_array().expect("batch response array");
            assert_eq!(items.len(), 2);
            assert!(
                items.iter().any(|item| {
                    item["id"] == 1
                        && item["error"]["code"] == -32603
                        && item["error"]["message"]
                            == "request handler dropped request without responding"
                }),
                "{items:?}"
            );
            assert!(
                items
                    .iter()
                    .any(|item| item["id"] == 2 && item["result"]["handled"] == "second"),
                "{items:?}"
            );

            drop(client);
        });
    }

    #[test]
    fn dropped_direct_request_without_runtime_still_emits_internal_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, server_read, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");
            let mut requests = client.take_requests().expect("request receiver");

            server_write
                .write_all(br#"{"jsonrpc":"2.0","id":1,"method":"first"}"#)
                .await
                .expect("write request");
            server_write.write_all(b"\n").await.expect("write newline");
            server_write.flush().await.expect("flush request");

            let request = tokio::time::timeout(Duration::from_secs(1), requests.recv())
                .await
                .expect("request timeout")
                .expect("request");

            (request, server_read, client)
        });
        drop(runtime);

        drop(request);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build read runtime");

        runtime.block_on(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();
            let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .expect("response timeout")
                .expect("read response")
                .expect("response line");
            let response: Value = serde_json::from_str(&response_line).expect("parse response");
            assert_eq!(response["id"], 1);
            assert_eq!(response["error"]["code"], -32603);
            assert_eq!(
                response["error"]["message"],
                "request handler dropped request without responding"
            );

            drop(client);
        });
    }

    #[tokio::test]
    async fn dropping_request_clone_does_not_release_batch_slot_early() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(
                br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
            )
            .await
            .expect("write batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush batch");

        let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("first request timeout")
            .expect("first request");
        let first_clone = first.clone();
        let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("second request timeout")
            .expect("second request");

        drop(first_clone);
        second
            .respond_ok(serde_json::json!({"handled":"second"}))
            .await
            .expect("respond second");
        first
            .respond_ok(serde_json::json!({"handled":"first"}))
            .await
            .expect("respond first");

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 2);

        drop(client);
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn max_message_bytes_zero_falls_back_to_default() {
        assert_eq!(
            normalize_max_message_bytes(0),
            Limits::default().max_message_bytes
        );
        assert_eq!(normalize_max_message_bytes(4096), 4096);
    }

    #[tokio::test]
    async fn invalid_json_line_closes_client_and_drains_pending_requests() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = Client::connect_io(client_read, client_write).await.unwrap();
        let handle = client.handle();
        let request_task = tokio::spawn({
            let client = handle.clone();
            async move { client.request("demo/ping", serde_json::json!({})).await }
        });

        let mut lines = tokio::io::BufReader::new(server_read).lines();
        let request_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("request timeout")
            .expect("read request")
            .expect("request line");
        let request: Value = serde_json::from_str(&request_line).expect("parse request");
        assert_eq!(request["method"], "demo/ping");

        server_write.write_all(b"not-json\n").await.unwrap();
        server_write.flush().await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if client.stats().invalid_json_lines >= 1 && client.is_closed() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let err = request_task
            .await
            .unwrap()
            .expect_err("request should fail closed");
        assert!(matches!(err, Error::Protocol(_)));
        assert!(
            handle
                .close_reason()
                .as_deref()
                .is_some_and(|reason: &str| reason.contains("invalid JSON line"))
        );
    }

    #[tokio::test]
    async fn invalid_jsonrpc_frame_closes_client_after_invalid_request_response() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = Client::connect_io(client_read, client_write).await.unwrap();
        let handle = client.handle();
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(br#"{"jsonrpc":"1.0","id":7,"method":"demo/callback"}"#)
            .await
            .unwrap();
        server_write.write_all(b"\n").await.unwrap();
        server_write.flush().await.unwrap();

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse response");
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["error"]["message"], "invalid jsonrpc version");

        tokio::time::timeout(Duration::from_secs(1), async {
            while !client.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let err = client
            .request("demo/ping", serde_json::json!({}))
            .await
            .expect_err("closed client should reject new requests");
        assert!(matches!(err, Error::Protocol(_)));
        assert!(
            handle
                .close_reason()
                .as_deref()
                .is_some_and(|reason: &str| reason.contains("invalid jsonrpc version"))
        );
    }

    #[test]
    fn spawn_detached_runs_tasks_without_tokio_runtime() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_task = Arc::clone(&counter);
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        spawn_detached("test detached runtime", async move {
            counter_for_task.fetch_add(1, AtomicOrdering::Relaxed);
            done_tx.send(()).unwrap();
        });

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("detached runtime should execute queued task");
        assert_eq!(counter.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn invalid_json_samples_keep_latest_lines_when_buffer_is_full() {
        let diagnostics = DiagnosticsState::new(&DiagnosticsOptions {
            invalid_json_sample_lines: 2,
            invalid_json_sample_max_bytes: 64,
        })
        .expect("diagnostics enabled");

        diagnostics.record_invalid_json_line(b"invalid-1");
        diagnostics.record_invalid_json_line(b"invalid-2");
        diagnostics.record_invalid_json_line(b"invalid-3");

        assert_eq!(
            diagnostics.invalid_json_samples(),
            vec!["invalid-2".to_string(), "invalid-3".to_string()]
        );
    }

    #[tokio::test]
    async fn notification_queue_overflow_closes_client_and_tracks_stats() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (_server_read, mut server_write) = tokio::io::split(server_stream);

        let mut options = SpawnOptions::default();
        options.limits.notifications_capacity = 1;
        let client = Client::connect_io_with_options(client_read, client_write, options)
            .await
            .unwrap();
        let handle = client.handle();

        let note = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "demo/notify",
            "params": {},
        });
        let mut out = serde_json::to_string(&note).unwrap();
        out.push('\n');
        server_write.write_all(out.as_bytes()).await.unwrap();
        server_write.write_all(out.as_bytes()).await.unwrap();
        server_write.flush().await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if handle.is_closed() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(client.stats().dropped_notifications_full, 1);
        assert!(
            handle
                .close_reason()
                .unwrap_or_default()
                .contains("notification queue is full")
        );
    }
}

#[cfg(test)]
#[cfg(unix)]
mod wait_timeout_tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct BlockingWrite {
        entered: Arc<AtomicBool>,
    }

    impl tokio::io::AsyncWrite for BlockingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }
    }

    #[tokio::test]
    async fn wait_with_timeout_kill_still_kills_when_close_stage_times_out() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg("exec sleep 10");

        let mut client = match Client::spawn_command(cmd).await {
            Ok(client) => client,
            Err(err) => panic!("spawn client: {err}"),
        };
        let entered = Arc::new(AtomicBool::new(false));
        {
            let mut write = client.handle.write.lock().await;
            *write = Box::new(BlockingWrite {
                entered: entered.clone(),
            });
        }

        let wait_result = client
            .wait_with_timeout(
                Duration::from_millis(20),
                WaitOnTimeout::Kill {
                    kill_timeout: Duration::from_secs(1),
                },
            )
            .await;
        let child_status = match wait_result {
            Ok(status) => status,
            Err(err) => panic!("wait should kill child even when close stage times out: {err}"),
        };
        let status = match child_status {
            Some(status) => status,
            None => panic!("child exit status"),
        };

        assert!(entered.load(Ordering::Relaxed));
        assert!(!status.success());
    }

    #[test]
    fn request_optional_with_timeout_returns_error_without_tokio_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let (client_stream, _server_stream) = tokio::io::duplex(1024);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let client = Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");

            let err = client
                .request_optional_with_timeout("demo/request", None, Duration::from_secs(1))
                .await
                .expect_err("missing time driver should fail");
            match err {
                Error::Protocol(protocol_err) => {
                    assert_eq!(protocol_err.kind, ProtocolErrorKind::Other);
                    assert!(protocol_err.message.contains("time driver"));
                }
                other => panic!("expected protocol error, got {other:?}"),
            }
        });
    }

    #[test]
    fn wait_with_timeout_returns_error_without_tokio_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let (client_stream, _server_stream) = tokio::io::duplex(1024);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let mut client = Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");

            let err = client
                .wait_with_timeout(Duration::from_secs(1), WaitOnTimeout::ReturnError)
                .await
                .expect_err("missing time driver should fail");
            match err {
                Error::Protocol(protocol_err) => {
                    assert_eq!(protocol_err.kind, ProtocolErrorKind::Other);
                    assert!(protocol_err.message.contains("time driver"));
                }
                other => panic!("expected protocol error, got {other:?}"),
            }
        });
    }
}

#[cfg(test)]
mod background_close_tests {
    use super::*;

    #[test]
    fn schedule_close_once_without_runtime_drains_pending_without_panic() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let handle = ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(
                Box::new(tokio::io::sink()) as Box<dyn AsyncWrite + Send + Unpin>
            )),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: pending.clone(),
            max_pending_requests: 1,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(OnceLock::new()),
            stdout_log_write_error: Arc::new(OnceLock::new()),
        };

        handle.schedule_close_once("closed outside runtime".to_string());

        assert!(handle.is_closed());
        assert_eq!(
            handle.close_reason().as_deref(),
            Some("closed outside runtime")
        );
        assert!(lock_pending(&pending).is_empty());

        let drained = rx
            .blocking_recv()
            .expect("pending request should be drained");
        let err = drained.expect_err("drained pending request must receive closed error");
        assert!(matches!(
            err,
            Error::Protocol(ProtocolError {
                kind: ProtocolErrorKind::Closed,
                ..
            })
        ));
    }
}

#[cfg(test)]
mod cancelled_request_ids_tests {
    use super::*;

    #[test]
    fn cancelled_request_id_eviction_preserves_latest_reinserted_id() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Integer(1);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        assert!(take_cancelled_request_id(&cancelled_request_ids, &id));

        // Reinsert the same id, then push enough unique ids to evict stale queue entries.
        remember_cancelled_request_id(&cancelled_request_ids, &id);
        for value in 2..=(CANCELLED_REQUEST_IDS_MAX as i64) {
            remember_cancelled_request_id(&cancelled_request_ids, &Id::Integer(value));
        }

        assert!(take_cancelled_request_id(&cancelled_request_ids, &id));
        assert!(!take_cancelled_request_id(&cancelled_request_ids, &id));

        let guard = lock_cancelled_request_ids(&cancelled_request_ids);
        assert!(guard.order.len() <= CANCELLED_REQUEST_IDS_MAX);
    }

    #[test]
    fn cancelled_request_id_type_mismatch_consumes_counterpart_entry() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Integer(7);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        assert!(take_cancelled_request_id_type_mismatch(
            &cancelled_request_ids,
            &Id::String("7".to_string())
        ));
        assert!(!take_cancelled_request_id(&cancelled_request_ids, &id));
    }

    #[test]
    fn cancelled_request_id_type_mismatch_consumes_large_unsigned_counterpart_entry() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Unsigned(u64::MAX);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        assert!(take_cancelled_request_id_type_mismatch(
            &cancelled_request_ids,
            &Id::String(u64::MAX.to_string())
        ));
        assert!(!take_cancelled_request_id(&cancelled_request_ids, &id));
    }

    #[test]
    fn cancelled_request_id_duplicate_insert_refreshes_recency() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Integer(1);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        for value in 2..=(CANCELLED_REQUEST_IDS_MAX as i64) {
            remember_cancelled_request_id(&cancelled_request_ids, &Id::Integer(value));
        }
        // Refresh the same id after the queue is full so its "latest generation" becomes recent.
        remember_cancelled_request_id(&cancelled_request_ids, &id);
        remember_cancelled_request_id(
            &cancelled_request_ids,
            &Id::Integer(CANCELLED_REQUEST_IDS_MAX as i64 + 1),
        );

        assert!(take_cancelled_request_id(&cancelled_request_ids, &id));
    }
}

#[cfg(test)]
mod response_routing_tests {
    use super::*;

    #[test]
    fn handle_response_routes_rpc_error_with_data() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));

        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32000,
                "message": "boom",
                "data": { "k": "v" }
            }
        });

        handle_response(&pending, &cancelled_request_ids, response).expect("handle response");

        let err = rx
            .blocking_recv()
            .expect("pending response channel")
            .expect_err("rpc error expected");
        match err {
            Error::Rpc {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32000);
                assert_eq!(message, "boom");
                assert_eq!(data, Some(serde_json::json!({ "k": "v" })));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn handle_response_rejects_result_and_error_together() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));

        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "ok": true },
            "error": {
                "code": -32000,
                "message": "boom"
            }
        });

        handle_response(&pending, &cancelled_request_ids, response).expect("handle response");

        let err = rx
            .blocking_recv()
            .expect("pending response channel")
            .expect_err("protocol error expected");
        match err {
            Error::Protocol(protocol_err) => {
                assert_eq!(protocol_err.kind, ProtocolErrorKind::InvalidMessage);
                assert!(
                    protocol_err
                        .message
                        .contains("must include exactly one of result/error")
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn handle_response_routes_large_unsigned_numeric_id() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));

        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Unsigned(u64::MAX), tx);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": u64::MAX,
            "result": { "ok": true }
        });

        handle_response(&pending, &cancelled_request_ids, response).expect("handle response");

        let result = rx
            .blocking_recv()
            .expect("pending response channel")
            .expect("result payload expected");
        assert_eq!(result, serde_json::json!({ "ok": true }));
        assert!(lock_pending(&pending).is_empty());
    }
}

#[cfg(test)]
mod error_record_tests {
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
