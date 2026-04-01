# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `mcp-jsonrpc`：`streamable_http` 的 SSE 桥接现在只接受 JSON-RPC object / batch array payload；单行垃圾文本或 JSON 标量会在 transport 边界直接 fail-closed，而不会再被写进内部 line-delimited JSON-RPC 通道。
- `mcp-jsonrpc`：`Client::close_in_background_once(...)` 现在会像显式关闭路径一样先 abort reader/transport tasks，并接管 child 的 best-effort kill/reap；`mcp-kit::Session::notify` 超时不再把后台任务和子进程留到 `Client::drop` 才收尾。
- `mcp-jsonrpc`：放宽 `streamable_http_reconnects_after_graceful_sse_eof` 回归测试的阶段等待窗口，避免完整 workspace 门禁负载下把 SSE 重连语义的真通过误判成超时假失败；不改变产品逻辑。
- `mcp-jsonrpc`：已有 Tokio runtime 的 dropped-request direct writeback 现在也带有单独的 1 秒收敛超时；如果底层 writer 永久 pending，连接会发布明确关闭原因并 fail-closed，而不是留下悬挂的 best-effort 写任务。
- `mcp-jsonrpc`：invalid-request / batch-invalid-request 的错误响应写回一旦失败，现在会立即 fail-closed 并保留真实 transport 关闭原因，而不是把写回失败静默吞掉后继续用协议错误掩盖响应丢失。
- `mcp-jsonrpc`：server→client request 响应写回一旦失败，现在会立刻发布关闭原因并 fail-closed 关闭 transport，而不是只把 I/O 错误返回给 handler 后继续把连接留在“看起来还活着”的状态。
- `mcp-jsonrpc`：`StdoutLog.max_parts` 的保留上限现在改成强约束；初始化和轮转阶段一旦无法完成 prune，就会直接报错而不是静默退化成 best-effort。
- `mcp-jsonrpc`：sync/no-runtime 的 dropped-request 与 batch 收尾不再偷偷启动 detached Tokio runtime；这些路径现在会显式 fail-closed 并发布关闭原因，而不是隐式补一个后台执行环境。
- `mcp-jsonrpc`：`streamable_http` 现在拒绝配置覆盖 transport 独占头 `Accept` / `Content-Type` / `mcp-session-id`，避免会话粘连和内容协商边界被调用方 headers 破坏。
- `mcp-jsonrpc`：所有通用出站写路径现在会在拿到 writer 锁后再次检查关闭状态，避免 close/drop 已发布后、排队中的 request/notify/response 仍继续写到底层 transport。
- `mcp-jsonrpc`：补充了 client drop 场景下 queued writer 的回归测试，确保 fail-closed 语义不只覆盖显式 close，也覆盖 `Client::drop` 的关闭发布路径。
- `mcp-jsonrpc`：关闭状态现在先发布 `close_reason` 再对外暴露 closed，可避免其他线程在 `streamable_http` 通知失败等关闭路径里先看到 `is_closed()`、却暂时读不到关闭原因。
- `mcp-jsonrpc`：把 transport 选项/limits 与公开错误边界分别下沉到 `transport.rs` 和 `error.rs`，`lib.rs` 只保留 client 主体与消息循环，减少单文件命名空间继续吞 transport/runtime 细节。
- `mcp-jsonrpc`：`streamable_http` 的独立 SSE 读侧现在会在正常 EOF 后自动重连，而不是把整个 transport 直接关闭；会 idle-close/轮换 SSE 的服务端不会再把客户端无谓打死。
- `mcp-jsonrpc`：`streamable_http` 的 SSE 唤醒信号改为无丢失传递，`SessionChanged` 不会再被排队中的 `Connect` 挤掉，活跃 SSE 在 session rollover 后会可靠切到新会话。
- `mcp-jsonrpc`：入站 server notification 在本地通知队列过载或接收端已关闭时不再静默丢弃；transport 现在会记录 stats 并主动关闭连接，把数据丢失显式暴露给调用方。
- `mcp-jsonrpc`：reader 现在对非法 JSON 和非法 JSON-RPC 帧 fail-closed；在记录诊断/返回 `invalid request` 后会立即关闭连接并清空 pending request，避免把协议损坏伪装成后续超时。
- `mcp-jsonrpc`：`streamable_http` 现在把 `server closed connection` 当作 generic EOF fallback；如果 SSE 先 EOF、随后 notify/POST 再暴露更具体的 HTTP 失败，公开 `close_reason()` 会升级到真实 transport 根因而不是卡在泛化原因上。
- `mcp-jsonrpc`：`request_optional_with_timeout` 与 `wait_with_timeout` 现在会在进入 `tokio::time` 前预检 time driver，把错误配置从 panic 收敛成稳定 `ProtocolError`，并在 API 文档中写明前提。
- `mcp-jsonrpc`：把 close-reason、cancelled-request bookkeeping、stats/diagnostics 等共享状态辅助逻辑下沉到内部 `state.rs`，让 `lib.rs` 更聚焦在 client 生命周期与消息循环，而不改变公开 API。
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- `mcp-jsonrpc`：`streamable_http` 现在会在初始 SSE 已建立后遇到新的 `mcp-session-id` 时主动切断旧 SSE 并按新 session 重连，避免 response/notification 继续挂在过期 session 上。
- `mcp-jsonrpc`：修复 active-SSE rollover 实现里的关闭回归，`Client::close()` abort transport tasks 时会同步终止当前 SSE pump，不再留下悬挂 SSE 连接阻塞关闭路径。
- `mcp-jsonrpc`：`streamable_http` 现在会在把同一个 SSE event 的多行 `data:` 写回内部 JSON-RPC 行流前先压平成单行 JSON，避免 pretty/multiline 响应被拆成半截消息。
- `mcp-jsonrpc`：所有通用出站写路径现在都会执行 `Limits.max_message_bytes` 限制，超大 request/response/batch 会在写入前以 `InvalidInput` 拒绝，避免不同 transport 对消息上限执行不一致。
- 提高了 `streamable_http` 长寿命 POST SSE timeout 回归测试的时间预算，保持“总时长超过 timeout 但持续有活动时不应失败”的语义，同时消除本地门禁的调度彩票。
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
- Dropping an unresponded `IncomingRequest` now emits a JSON-RPC internal error for direct and batch requests while a Tokio runtime is available, so peers do not hang waiting for a missing response during normal async handling.
- Added regression coverage for the sync/no-runtime drop path so dropping a handler-owned request outside a current Tokio runtime now closes the transport fail-closed with an explicit reason instead of spinning hidden background runtime cleanup.
- Added regression coverage for the `streamable_http` path where an already-open SSE stream must drop the stale connection, reconnect after a POST response rolls the session id, and continue delivering server notifications on the new session.
