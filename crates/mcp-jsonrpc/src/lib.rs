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

mod client;
mod detached;
mod error;
mod options;
mod reader;
mod runtime;
mod stdout_log;
mod streamable_http;

pub use client::{
    Client, ClientHandle, ClientStats, Id, IncomingRequest, Notification, WaitOnTimeout,
};
pub use error::{Error, ProtocolError, ProtocolErrorKind};
pub use options::{
    DiagnosticsOptions, Limits, SpawnOptions, StdoutLog, StdoutLogRedactor, StreamableHttpOptions,
    StreamableHttpProxyMode,
};

pub(crate) use client::{
    BatchResponseWriter, CancelledRequestIds, ClientStatsInner, CloseReasonPriority,
    DiagnosticsState, PendingRequests, RequestResponder, drain_pending, error_response_id_or_null,
    handle_response, lock_pending, outbound_error_response_value, parse_id_owned,
};
#[cfg(test)]
pub(crate) use options::DEFAULT_MAX_MESSAGE_BYTES;
pub(crate) use options::normalize_max_message_bytes;
pub(crate) use reader::{is_ascii_whitespace_only, read_line_limited, read_line_limited_into};
pub(crate) use runtime::ensure_tokio_time_driver;
