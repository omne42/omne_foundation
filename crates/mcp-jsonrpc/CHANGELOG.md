# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `mcp-jsonrpc`：`request_optional_with_timeout` 与 `wait_with_timeout` 现在会在进入 `tokio::time` 前预检 time driver，把错误配置从 panic 收敛成稳定 `ProtocolError`，并在 API 文档中写明前提。
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- `mcp-jsonrpc`：`streamable_http` 现在会在初始 SSE 已建立后遇到新的 `mcp-session-id` 时主动切断旧 SSE 并按新 session 重连，避免 response/notification 继续挂在过期 session 上。
- `mcp-jsonrpc`：修复 active-SSE rollover 实现里的关闭回归，`Client::close()` abort transport tasks 时会同步终止当前 SSE pump，不再留下悬挂 SSE 连接阻塞关闭路径。
- `mcp-jsonrpc`：`streamable_http` 现在会在把同一个 SSE event 的多行 `data:` 写回内部 JSON-RPC 行流前先压平成单行 JSON，避免 pretty/multiline 响应被拆成半截消息。
- Locked the multiline SSE normalization path behind crate-local regression coverage so future refactors do not reintroduce line-delimited JSON-RPC framing splits.
- Exposed `mcp-jsonrpc::Error` as a stable `error-kit::ErrorRecord` mapping with machine-readable error codes, categories, and retry advice.
- Added a regression test that keeps `streamable_http` long-lived POST SSE responses green when they outlive `request_timeout` but continue producing events.
- `mcp-jsonrpc`：`streamable_http` 现在支持把 untrusted transport 的 DNS 预检结果绑定到实际 HTTP socket，避免“先校验、后重解析”的 rebinding/TOCTOU 绕过。
- `mcp-jsonrpc`：`streamable_http` 的 request/body timeout 现在会直接回填为 `ProtocolErrorKind::WaitTimeout`，不再桥接成伪造的 `-32000` RPC server error。
- `mcp-jsonrpc`：`streamable_http` 不再把 `request_timeout` 当作整个 `text/event-stream` POST 响应的总时长上限，长时间持续产出的 SSE 响应不会被误杀。
- Treat malformed nested JSON-RPC batch items as `invalid request` errors instead of flattening them into normal dispatch.
- `mcp-jsonrpc`：`streamable_http` 现在复用 `http-kit::HttpClientProfile`，把可复用 HTTP 配置显式绑定到 pinned/unpinned 两条路径，避免依赖 opaque `reqwest::Client` 状态。
- `mcp-jsonrpc` 现在接受并路由超出 `i64` 范围的合法 unsigned numeric JSON-RPC `id`，避免把大整数请求/响应误判为无效消息。
- Rewrote the timeout child-kill branch without `let` chains so the crate remains compatible with the Rust 1.85 toolchain enforced by workspace gates.
- Aggregate top-level JSON-RPC batch responses into a single array so server->client requests received in a batch no longer emit protocol-invalid standalone response objects.
- Dropping an unresponded `IncomingRequest` now emits a JSON-RPC internal error for both direct and batch requests, so the peer never hangs waiting for a missing response and batch flushes still complete.
- Added regression coverage for the `streamable_http` path where an already-open SSE stream must reconnect after a POST response rolls the session id.
