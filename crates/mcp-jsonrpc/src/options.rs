use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub type StdoutLogRedactor = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

pub(crate) const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

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
    /// Drop also starts a background reap path so killed children do not linger as zombies, but
    /// explicit `Client::wait*` calls are still the preferred lifecycle boundary when you own the
    /// child process.
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StreamableHttpProxyMode {
    /// Ignore proxy environment variables such as `HTTP_PROXY` / `HTTPS_PROXY`.
    ///
    /// This is the safer default for untrusted or generic streamable HTTP transports.
    #[default]
    IgnoreSystem,
    /// Allow `reqwest` to read the process proxy environment.
    ///
    /// Note: when `StreamableHttpOptions.enforce_public_ip` is true, the pinned public-IP path
    /// still disables proxies so the socket cannot be redirected to an intermediate endpoint.
    UseSystem,
}

#[derive(Debug, Clone)]
pub struct StreamableHttpOptions {
    /// Extra HTTP headers to include on all requests.
    pub headers: HashMap<String, String>,
    /// Whether untrusted transports must pin the validated public IP set into the actual socket.
    pub enforce_public_ip: bool,
    /// Proxy environment loading policy for the unpinned HTTP client path.
    pub proxy_mode: StreamableHttpProxyMode,
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
            proxy_mode: StreamableHttpProxyMode::IgnoreSystem,
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
