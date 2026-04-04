#![forbid(unsafe_code)]

//! `mcp-kit` is a small, reusable MCP client toolkit.
//!
//! It provides:
//! - `Config`: loads and validates `mcp.json` (v1) from a workspace root.
//! - `Manager`: connection cache + MCP initialize + convenience `request` helpers.
//! - `Session`: a single initialized MCP connection that can be handed to other libraries.
//! - `shared::SharedManager`: cloneable single-flight wrapper for serialized async access to
//!   `Manager`.
//! - `mcp`: minimal typed wrappers for common MCP methods (subset of schema).
//!
//! ## Remote-first, safe-by-default
//!
//! Most MCP servers are remote. This crate supports remote servers natively via
//! `transport=streamable_http` (HTTP SSE + POST).
//!
//! Local transports (`transport=stdio|unix`) are powerful and potentially unsafe when a config
//! comes from an untrusted repository. Therefore `Manager` defaults to `TrustMode::Untrusted`:
//! - Refuses local `stdio`/`unix` transports
//! - Refuses arbitrary public `streamable_http` hosts unless you explicitly allowlist them (or
//!   opt into `allow_public_hosts=true`)
//! - Refuses caller-provided custom HTTP headers unless you explicitly opt in
//! - Refuses spawning processes (`stdio`) and connecting arbitrary unix sockets (`unix`)
//!
//! To fully trust the local configuration, explicitly opt in:
//! `Manager::with_trust_mode(TrustMode::Trusted)`.
//!
//! If you want to keep the default "untrusted" stance but relax/tighten remote egress checks,
//! configure `Manager::with_untrusted_streamable_http_policy(UntrustedStreamableHttpPolicy)`.
//!
//! `shared::SharedManager` is intentionally a single-flight wrapper around `Manager`, not an
//! actor. It serializes manager state mutations across clones and also uses same-server
//! lifecycle gates so cold-start connect/disconnect/`disconnect_and_wait` paths cannot overlap.
//! Request/notify operations release the shared manager lock once they have borrowed a
//! `ClientHandle`, but they keep the same-server lifecycle gate in read mode until the
//! corresponding JSON-RPC I/O finishes. That lets same-server request/notify traffic overlap
//! while still blocking concurrent same-server `disconnect` from tearing down the borrowed
//! connection underneath in-flight RPCs, including the cold-start config-driven path after the
//! freshly installed connection has been prepared. Operations that still require the shared lock
//! or lifecycle gate fail fast only when they are called reentrantly from manager-owned handlers
//! (or child tasks that explicitly inherit that scope) and would otherwise deadlock. Prefer plain
//! `Manager` when you need fine-grained lifecycle control or handler callbacks that may need to
//! call back into connection setup/teardown paths. If a handler must spawn a child task that calls
//! back into `shared::SharedManager`, use
//! `shared::SharedManager::spawn_inheriting_handler_scope(...)`; bare `tokio::spawn(...)` keeps
//! the normal external-caller waiting behavior because it does not inherit the handler task-local
//! reentrancy scope automatically.
//!
//! Direct manager connection APIs (`Manager::connect`, `connect_named`, transport helpers, etc.)
//! require an absolute `cwd`. They no longer fall back to the ambient process working directory.
//!
//! When you use config-driven connection helpers (`Manager::request`, `get_or_connect`, etc.),
//! relative `cwd` values are resolved against the loaded `mcp.json` thread root when available,
//! not against the ambient process directory. Those relative `cwd` values must stay lexical
//! descendants of that explicit base: `.` / `..` segments are rejected instead of being
//! normalized through the base/thread-root boundary.
//!
//! Config-driven helpers may reopen a cached server name after the old transport has already
//! closed, but this is only an on-demand reconnect inside the current `Manager`/`SharedManager`.
//! `mcp-kit` still does not provide background keepalive, daemonization, or retry policy.
//!
//! `mcp-kit` remains a runtime-first boundary: config loading/model types and transport/session
//! lifecycle still live in the same crate. If you only need generic config loading primitives,
//! prefer `config-kit`; `mcp-kit` keeps the MCP-specific contract coupled to the runtime stack on
//! purpose until a narrower split proves stable.
//!
//! ## Non-goals
//!
//! - Implementing an MCP server
//! - High-level policies (approvals/sandbox/tool execution strategy)
//! - Background reconnect policy/daemonization

mod config;
mod convenience;
mod error;
mod manager;
pub mod mcp;
mod protocol;
mod security;
mod server_name;
mod session;
mod shared_manager;
pub mod shared {
    pub use crate::shared_manager::SharedManager;
}

pub use config::{
    ClientConfig, Config, ConfigLoadPolicy, Root, ServerConfig, StdoutLogConfig, Transport,
};
pub use error::{Error, ErrorKind, Result};
pub use manager::{
    Connection, Manager, ProtocolVersionCheck, ProtocolVersionMismatch, ServerNotificationContext,
    ServerNotificationHandler, ServerRequestContext, ServerRequestHandler, ServerRequestOutcome,
};
pub use protocol::{MCP_PROTOCOL_VERSION, McpNotification, McpRequest};
pub use security::{TrustMode, UntrustedStreamableHttpPolicy};
pub use server_name::{ServerName, ServerNameError};
pub use session::Session;
