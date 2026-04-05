use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use crate::detached::{close_without_runtime, spawn_detached};
use crate::reader::{ReaderTaskContext, spawn_reader_task};
use crate::stdout_log::LogState;
use crate::{
    Error, ProtocolErrorKind, SpawnOptions, ensure_tokio_time_driver, normalize_max_message_bytes,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum CloseReasonPriority {
    Fallback,
    Primary,
}

#[derive(Debug, Default)]
pub(crate) struct CloseReasonState {
    inner: Mutex<Option<(CloseReasonPriority, String)>>,
}

impl CloseReasonState {
    pub(crate) fn publish(&self, priority: CloseReasonPriority, reason: String) -> bool {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let should_replace = match guard.as_ref() {
            None => true,
            Some((current_priority, _)) => priority > *current_priority,
        };
        if should_replace {
            *guard = Some((priority, reason));
        }
        should_replace
    }

    fn get(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|(_, reason)| reason.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Id {
    String(String),
    Integer(i64),
    Unsigned(u64),
}

pub(crate) type PendingRequests = Arc<Mutex<HashMap<Id, oneshot::Sender<Result<Value, Error>>>>>;
pub(crate) type CancelledRequestIds = Arc<Mutex<CancelledRequestIdsState>>;

const CANCELLED_REQUEST_IDS_MAX: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct CancelledRequestIdsState {
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
pub(crate) struct ClientStatsInner {
    pub(crate) invalid_json_lines: AtomicU64,
    pub(crate) dropped_notifications_full: AtomicU64,
    pub(crate) dropped_notifications_closed: AtomicU64,
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
pub(crate) struct DiagnosticsState {
    invalid_json_samples: Mutex<VecDeque<String>>,
    invalid_json_sample_lines: usize,
    invalid_json_sample_max_bytes: usize,
}

impl DiagnosticsState {
    pub(crate) fn new(opts: &crate::DiagnosticsOptions) -> Option<Arc<Self>> {
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

    pub(crate) fn record_invalid_json_line(&self, line: &[u8]) {
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

    pub(crate) fn invalid_json_samples(&self) -> Vec<String> {
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
    pub(crate) write: Arc<tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>>,
    max_message_bytes: usize,
    next_id: Arc<AtomicI64>,
    pub(crate) pending: PendingRequests,
    max_pending_requests: usize,
    cancelled_request_ids: CancelledRequestIds,
    stats: Arc<ClientStatsInner>,
    diagnostics: Option<Arc<DiagnosticsState>>,
    pub(crate) closed: Arc<AtomicBool>,
    pub(crate) close_reason: Arc<CloseReasonState>,
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

fn ensure_outbound_message_size_within_limit(
    line: &[u8],
    max_message_bytes: usize,
) -> Result<(), Error> {
    let payload_len = line
        .len()
        .saturating_sub(usize::from(line.ends_with(b"\n")));
    if payload_len <= max_message_bytes {
        return Ok(());
    }

    Err(Error::protocol(
        ProtocolErrorKind::InvalidInput,
        format!(
            "jsonrpc message too large (max_bytes={max_message_bytes} actual_bytes={payload_len})"
        ),
    ))
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

pub(crate) fn outbound_error_response_value<I>(
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
pub(crate) struct BatchResponseWriter {
    state: Arc<BatchResponseState>,
}

impl std::fmt::Debug for BatchResponseWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchResponseWriter")
            .finish_non_exhaustive()
    }
}

impl BatchResponseWriter {
    pub(crate) fn new(handle: ClientHandle) -> Self {
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

    pub(crate) async fn push_immediate_response(&self, response: Value) -> Result<(), Error> {
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
        if let Err(err) = spawn_detached("batch flush without runtime", async move {
            let _ = batch.flush_if_ready().await;
        }) {
            close_without_runtime(&self.state.handle, err.close_reason());
        }
    }

    pub(crate) async fn finish(&self) -> Result<(), Error> {
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
pub(crate) struct RequestResponder {
    target: RequestResponseTarget,
    responded: Arc<AtomicBool>,
}

impl RequestResponder {
    pub(crate) fn direct(handle: ClientHandle) -> Self {
        Self {
            target: RequestResponseTarget::Direct(handle),
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn batch(batch: BatchResponseWriter) -> Self {
        Self {
            target: RequestResponseTarget::Batch(batch.reserve_request_slot()),
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn respond_ok(&self, id: &Id, result: Value) -> Result<(), Error> {
        let response = outbound_ok_response_value(id, &result)?;
        self.send_response(response).await
    }

    pub(crate) async fn respond_error(
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

    pub fn close_reason(&self) -> Option<String> {
        self.close_reason.get()
    }

    /// Returns the last stdout log write error, if any.
    ///
    /// When this is set, the client disables stdout log writes for the remainder of its
    /// lifetime. This is not treated as a fatal transport error.
    pub fn stdout_log_write_error(&self) -> Option<String> {
        self.stdout_log_write_error.get().cloned()
    }

    pub(crate) fn record_stdout_log_write_error(&self, err: &std::io::Error) {
        let _ = self.stdout_log_write_error.set(err.to_string());
    }

    pub async fn close(&self, reason: impl Into<String>) {
        self.close_with_reason(reason).await;
    }

    pub(crate) fn schedule_close_once(&self, reason: String) {
        if self
            .closed
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        self.close_reason
            .publish(CloseReasonPriority::Primary, reason.clone());
        let handle = self.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(async move {
                handle.close_with_reason(reason).await;
            }));
            return;
        }

        // No runtime available (e.g. sync context): avoid panicking and perform best-effort close.
        close_without_runtime(&handle, reason);
    }

    pub(crate) fn check_closed(&self) -> Result<(), Error> {
        if !self.closed.load(Ordering::Relaxed) {
            return Ok(());
        }
        let reason = self
            .close_reason
            .get()
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

        self.close_reason
            .publish(CloseReasonPriority::Primary, reason);
        self.closed.store(true, Ordering::Relaxed);

        drain_pending(&self.pending, &err);
        let mut write = self.write.lock().await;
        let _ = write.shutdown().await;
        // Many `AsyncWrite` impls (e.g. `tokio::process::ChildStdin`) only fully close on drop.
        // Replacing the writer guarantees the underlying write end is closed.
        let _ = std::mem::replace(&mut *write, Box::new(tokio::io::sink()));
    }

    fn fail_closed_after_write_error(
        &self,
        write: &mut Box<dyn AsyncWrite + Send + Unpin>,
        err: &std::io::Error,
    ) {
        let reason = format!("json-rpc transport write failed: {err}");
        if self
            .close_reason
            .publish(CloseReasonPriority::Primary, reason.clone())
        {
            self.closed.store(true, Ordering::Relaxed);
            drain_pending(
                &self.pending,
                &Error::protocol(ProtocolErrorKind::Closed, reason),
            );
        } else {
            self.closed.store(true, Ordering::Relaxed);
        }
        let _ = std::mem::replace(write, Box::new(tokio::io::sink()));
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

    pub(crate) async fn write_json_line(&self, value: &impl Serialize) -> Result<(), Error> {
        let line = serialize_json_line(value)?;
        ensure_outbound_message_size_within_limit(&line, self.max_message_bytes)?;
        self.write_line(&line).await
    }

    async fn write_line(&self, line: &[u8]) -> Result<(), Error> {
        self.check_closed()?;
        let mut write = self.write.lock().await;
        if let Err(err) = write.write_all(line).await {
            self.fail_closed_after_write_error(&mut write, &err);
            return Err(Error::Io(err));
        }
        if let Err(err) = write.flush().await {
            self.fail_closed_after_write_error(&mut write, &err);
            return Err(Error::Io(err));
        }
        drop(write);
        Ok(())
    }
}

pub struct Client {
    pub(crate) handle: ClientHandle,
    child: Option<Child>,
    notifications_rx: Option<mpsc::Receiver<Notification>>,
    requests_rx: Option<mpsc::Receiver<IncomingRequest>>,
    task: tokio::task::JoinHandle<()>,
    pub(crate) transport_tasks: Vec<tokio::task::JoinHandle<()>>,
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
        let max_message_bytes = normalize_max_message_bytes(limits.max_message_bytes);
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
            max_message_bytes,
            next_id: Arc::new(AtomicI64::new(1)),
            pending: pending.clone(),
            max_pending_requests,
            cancelled_request_ids: cancelled_request_ids.clone(),
            stats: stats.clone(),
            diagnostics: diagnostics_state.clone(),
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(CloseReasonState::default()),
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
        self.handle
            .close_reason
            .publish(CloseReasonPriority::Fallback, "client closed".to_string());
        self.handle.closed.store(true, Ordering::Relaxed);
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
    pub(crate) responder: RequestResponder,
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
                let handle_for_task = handle.clone();
                if let Err(err) = spawn_detached("direct dropped request response", async move {
                    drop(handle_for_task.write_json_line(&response).await);
                }) {
                    close_without_runtime(&handle, err.close_reason());
                }
            }
            RequestResponseTarget::Batch(batch) => {
                if tokio::runtime::Handle::try_current().is_ok() {
                    let batch = batch.clone();
                    let batch_for_task = batch.clone();
                    if let Err(err) = spawn_detached("batch dropped request response", async move {
                        drop(batch_for_task.push_reserved_response(response).await);
                    }) {
                        close_without_runtime(&batch.state.handle, err.close_reason());
                    }
                } else {
                    batch.push_reserved_response_without_runtime(response);
                }
            }
        }
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

pub(crate) fn lock_pending(
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

pub(crate) fn drain_pending(pending: &PendingRequests, err: &Error) {
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

pub(crate) fn error_response_id_or_null(value: Value) -> Value {
    match value {
        Value::String(_) | Value::Number(_) => value,
        _ => Value::Null,
    }
}

pub(crate) fn parse_id_owned(value: Value) -> Option<Id> {
    match value {
        Value::String(value) => Some(Id::String(value)),
        Value::Number(value) => value
            .as_i64()
            .map(Id::Integer)
            .or_else(|| value.as_u64().map(Id::Unsigned)),
        _ => None,
    }
}

pub(crate) fn handle_response(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProtocolError;
    use crate::detached::{detached_runtime_test_guard, force_detached_init_failures};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
    use std::task::{Context, Poll};
    use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

    #[test]
    fn max_message_bytes_zero_falls_back_to_default() {
        assert_eq!(
            normalize_max_message_bytes(0),
            crate::Limits::default().max_message_bytes
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

    #[test]
    fn spawn_detached_runs_tasks_without_tokio_runtime() {
        let _guard = detached_runtime_test_guard();
        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_task = Arc::clone(&counter);
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        spawn_detached("test detached runtime", async move {
            counter_for_task.fetch_add(1, AtomicOrdering::Relaxed);
            done_tx.send(()).unwrap();
        })
        .expect("detached runtime should accept queued task");

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("detached runtime should execute queued task");
        assert_eq!(counter.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn spawn_detached_returns_error_when_detached_runtime_init_fails() {
        let _guard = detached_runtime_test_guard();
        force_detached_init_failures(1);

        let err = spawn_detached("test detached runtime init failure", async {})
            .expect_err("forced init failure should not panic");
        assert!(
            err.close_reason()
                .contains("detached runtime unavailable for test detached runtime init failure"),
            "{err}"
        );
    }

    #[test]
    fn batch_flush_without_runtime_closes_handle_when_detached_runtime_is_unavailable() {
        let _guard = detached_runtime_test_guard();
        force_detached_init_failures(1);

        let handle = ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(
                Box::new(tokio::io::sink()) as Box<dyn AsyncWrite + Send + Unpin>
            )),
            max_message_bytes: crate::DEFAULT_MAX_MESSAGE_BYTES,
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            max_pending_requests: 1,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(CloseReasonState::default()),
            stdout_log_write_error: Arc::new(OnceLock::new()),
        };
        let batch = BatchResponseWriter::new(handle.clone()).reserve_request_slot();
        batch.state.finished.store(true, Ordering::Relaxed);

        batch.push_reserved_response_without_runtime(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "ok": true }
        }));

        assert!(
            handle.is_closed(),
            "detached runtime failure must fail closed"
        );
        assert!(
            handle
                .close_reason()
                .as_deref()
                .is_some_and(|reason| reason.contains("batch flush without runtime")),
            "{:?}",
            handle.close_reason()
        );
    }

    #[test]
    fn invalid_json_samples_keep_latest_lines_when_buffer_is_full() {
        let diagnostics = DiagnosticsState::new(&crate::DiagnosticsOptions {
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

    #[cfg(unix)]
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

    #[test]
    fn schedule_close_once_without_runtime_drains_pending_without_panic() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let handle = ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(
                Box::new(tokio::io::sink()) as Box<dyn AsyncWrite + Send + Unpin>
            )),
            max_message_bytes: crate::DEFAULT_MAX_MESSAGE_BYTES,
            next_id: Arc::new(AtomicI64::new(1)),
            pending: pending.clone(),
            max_pending_requests: 1,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(CloseReasonState::default()),
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
