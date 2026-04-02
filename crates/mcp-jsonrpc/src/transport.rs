use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub type StdoutLogRedactor = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct SpawnOptions {
    pub stdout_log: Option<StdoutLog>,
    pub stdout_log_redactor: Option<StdoutLogRedactor>,
    pub limits: Limits,
    pub diagnostics: DiagnosticsOptions,
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
    /// Extra HTTP headers to include on requests.
    ///
    /// `streamable_http` transport-owned headers such as `Accept`, `Content-Type`, and
    /// `mcp-session-id` are reserved and rejected at connect time.
    pub headers: HashMap<String, String>,
    pub enforce_public_ip: bool,
    pub connect_timeout: Option<Duration>,
    /// Bounds POST request setup, waiting for the initial HTTP response, and non-SSE body reads.
    ///
    /// Once a POST has successfully returned `text/event-stream` response headers, the remaining
    /// SSE body is pumped without this timeout. Use a different upper bound if callers need to cap
    /// total end-to-end streaming duration.
    pub request_timeout: Option<Duration>,
    pub follow_redirects: bool,
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
    pub invalid_json_sample_lines: usize,
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
    pub max_parts: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_message_bytes: usize,
    pub notifications_capacity: usize,
    pub requests_capacity: usize,
    pub max_pending_requests: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
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
