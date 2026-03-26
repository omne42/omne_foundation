# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Exposed `mcp-jsonrpc::Error` as a stable `error-kit::ErrorRecord` mapping with machine-readable error codes, categories, and retry advice.
- Added a regression test that keeps `streamable_http` long-lived POST SSE responses green when they outlive `request_timeout` but continue producing events.
- `mcp-jsonrpc`：`streamable_http` 现在支持把 untrusted transport 的 DNS 预检结果绑定到实际 HTTP socket，避免“先校验、后重解析”的 rebinding/TOCTOU 绕过。
- `mcp-jsonrpc`：`streamable_http` 的 request/body timeout 现在会直接回填为 `ProtocolErrorKind::WaitTimeout`，不再桥接成伪造的 `-32000` RPC server error。
- `mcp-jsonrpc`：`streamable_http` 不再把 `request_timeout` 当作整个 `text/event-stream` POST 响应的总时长上限，长时间持续产出的 SSE 响应不会被误杀。
- Treat malformed nested JSON-RPC batch items as `invalid request` errors instead of flattening them into normal dispatch.
- `mcp-jsonrpc`：`streamable_http` 现在复用 `http-kit::HttpClientProfile`，把可复用 HTTP 配置显式绑定到 pinned/unpinned 两条路径，避免依赖 opaque `reqwest::Client` 状态。
- `mcp-jsonrpc` 现在接受并路由超出 `i64` 范围的合法 unsigned numeric JSON-RPC `id`，避免把大整数请求/响应误判为无效消息。
- Rewrote the timeout child-kill branch without `let` chains so the crate remains compatible with the Rust 1.85 toolchain enforced by workspace gates.
