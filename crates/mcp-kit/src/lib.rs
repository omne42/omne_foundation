#![forbid(unsafe_code)]

//! `mcp-kit` is a small, reusable MCP client toolkit.
//!
//! It provides:
//! - `Config`: loads and validates `mcp.json` (v1) from a workspace root.
//! - `Manager`: connection cache + MCP initialize + convenience `request` helpers.
//! - `SharedManager`: cloneable single-flight wrapper for serialized async access to `Manager`.
//! - `Session`: a single initialized MCP connection that can be handed to other libraries.
//! - `mcp`: minimal typed wrappers for common MCP methods (subset of schema).
//!
//! ## Remote-first, safe-by-default
//!
//! Most MCP servers are remote. This crate supports remote servers natively via
//! `transport=streamable_http` (HTTP SSE + POST).
//!
//! Local transports (`transport=stdio|unix`) are powerful and potentially unsafe when a config
//! comes from an untrusted repository. Therefore `Manager` defaults to `TrustMode::Untrusted`:
//! - Allows remote `streamable_http` connections (but refuses reading env secrets for auth headers)
//! - Refuses spawning processes (`stdio`) and connecting arbitrary unix sockets (`unix`)
//!
//! To fully trust the local configuration, explicitly opt in:
//! `Manager::with_trust_mode(TrustMode::Trusted)`.
//!
//! If you want to keep the default "untrusted" stance but relax/tighten remote egress checks,
//! configure `Manager::with_untrusted_streamable_http_policy(UntrustedStreamableHttpPolicy)`.
//!
//! `SharedManager` is intentionally a single-flight wrapper around `Manager`, not an actor. It
//! serializes manager state mutations across clones and also uses same-server lifecycle gates so
//! cold-start connect/disconnect/`disconnect_and_wait` paths cannot overlap. Connected
//! request/notify operations release the manager lock before awaiting JSON-RPC I/O, while
//! operations that still require the shared lock or lifecycle gate fail fast when they are called
//! reentrantly from manager-owned handlers and would otherwise deadlock. Prefer plain `Manager`
//! when you need fine-grained lifecycle control or handler callbacks that may need to call back
//! into connection setup/teardown paths.
//!
//! When you use config-driven connection helpers (`Manager::request`, `get_or_connect`, etc.),
//! relative `cwd` values are resolved against the loaded `mcp.json` thread root when available,
//! not against the ambient process directory.
//!
//! When you use config-driven connection helpers (`Manager::request`, `get_or_connect`, etc.),
//! relative `cwd` values are resolved against the loaded `mcp.json` thread root when available,
//! not against the ambient process directory.
//!
//! ## Non-goals
//!
//! - Implementing an MCP server
//! - High-level policies (approvals/sandbox/tool execution strategy)
//! - Automatic reconnect/daemonization

mod config;
mod manager;
pub mod mcp;
mod protocol;
mod security;
mod server_name;
mod session;
mod shared_manager;

pub use config::{
    ClientConfig, Config, ConfigLoadPolicy, Root, ServerConfig, StdoutLogConfig, Transport,
};
pub use manager::{
    Connection, Manager, ProtocolVersionCheck, ProtocolVersionMismatch, ServerNotificationContext,
    ServerNotificationHandler, ServerRequestContext, ServerRequestHandler, ServerRequestOutcome,
};
pub use protocol::{MCP_PROTOCOL_VERSION, McpNotification, McpRequest};
pub use security::{TrustMode, UntrustedStreamableHttpPolicy};
pub use server_name::{ServerName, ServerNameError};
pub use session::Session;
pub use shared_manager::SharedManager;
