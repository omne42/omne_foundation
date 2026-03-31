use serde::Serialize;
use serde::de::DeserializeOwned;

/// MCP protocol version used during `initialize`.
///
/// This matches the MCP schema version used by the upstream MCP specification.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub(crate) const MCP_PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";
pub(crate) const AUTHORIZATION_HEADER: &str = "Authorization";
pub(crate) const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

pub(crate) fn is_reserved_streamable_http_transport_header(header: &str) -> bool {
    header.eq_ignore_ascii_case(MCP_PROTOCOL_VERSION_HEADER)
        || header.eq_ignore_ascii_case(AUTHORIZATION_HEADER)
        || header.eq_ignore_ascii_case(MCP_SESSION_ID_HEADER)
        || header.eq_ignore_ascii_case("accept")
        || header.eq_ignore_ascii_case("content-type")
}

/// Typed MCP request (method + params + result).
///
/// This is a lightweight, schema-agnostic abstraction inspired by common MCP typed wrappers.
pub trait McpRequest {
    const METHOD: &'static str;
    type Params: Serialize;
    type Result: DeserializeOwned;
}

/// Typed MCP notification (method + params).
///
/// This is a lightweight, schema-agnostic abstraction inspired by common MCP typed wrappers.
pub trait McpNotification {
    const METHOD: &'static str;
    type Params: Serialize;
}
