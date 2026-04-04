# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `mcp-jsonrpc`：底层 `write_all` / `flush` 一旦报错，现在会立即 fail-closed 标记连接已坏、drain 掉全部 pending request，并替换写端；坏 transport 不会再继续暴露成“看起来还活着”的 client。
- `mcp-jsonrpc`：`spawn_command*` 路径上的 `Client::Drop` 现在会在 `kill_on_drop=true` 时显式触发后台 reap，避免子进程被杀掉后仍延迟停留为 zombie；显式 `wait*` 仍是首选生命周期边界。
- `mcp-jsonrpc`：`stdout_log.max_parts` 的轮转清理现在对删除失败 fail-closed，初始化和后续 rotation 都会把 prune 错误显式返回，而不是静默把保留策略降级成 best-effort。
- `mcp-jsonrpc`：sync/no-runtime 的 detached fallback 现在收口到独立 `detached` 模块，并显式启用共享多线程 Tokio runtime handle 调度后台收尾任务；一个卡住的 dropped-request writeback 或 batch flush 不会再把后续 detached 补偿任务串行堵死。
- `mcp-jsonrpc`：当 detached fallback 不可用时，batch flush、dropped-request response 和 close cleanup 现在都会显式 fail-closed 关闭 transport 并发布关闭原因，而不是 panic 或静默吞掉后台任务。
- `mcp-jsonrpc`：`streamable_http` 的 graceful SSE EOF 重连现在带最小间隔、轻量抖动并按连续 EOF 指数回退，避免在慢速/macOS 环境里与对端的优雅收尾互相踩出早退重连；session rollover 触发的主动 SSE 切换仍保持立即重连。
- `mcp-jsonrpc`：加固了 graceful SSE EOF 重连回归测试，在初始 SSE 头之后先发送一条注释帧再优雅收尾，并放宽阶段等待预算，避免慢速/macOS CI 上把“EOF 后应重连”的真通过误判成超时。
- `mcp-jsonrpc`：放宽 graceful SSE EOF 重连回归测试里等待第二次 SSE 建连的超时预算，降低慢速 CI runner 上的时序抖动假阴性，同时继续锁住“EOF 后必须重连”这条契约。
- `mcp-jsonrpc`：`streamable_http` 的独立 SSE 读侧现在会在正常 EOF 后自动重连，而不是把整个 transport 直接关闭；会 idle-close/轮换 SSE 的服务端不会再把客户端无谓打死。
- `mcp-jsonrpc`：`streamable_http` 的 SSE 唤醒信号改为无丢失传递，`SessionChanged` 不会再被排队中的 `Connect` 挤掉，活跃 SSE 在 session rollover 后会可靠切到新会话。
- `mcp-jsonrpc`：入站 server notification 在本地通知队列过载或接收端已关闭时不再静默丢弃；transport 现在会记录 stats 并主动关闭连接，把数据丢失显式暴露给调用方。
- `mcp-jsonrpc`：reader 现在对非法 JSON 和非法 JSON-RPC 帧 fail-closed；在记录诊断/返回 `invalid request` 后会立即关闭连接并清空 pending request，避免把协议损坏伪装成后续超时。
- `mcp-jsonrpc`：同步/无 Tokio runtime 的 dropped-request 与 batch flush 补偿路径现在复用单例后台 runtime，而不是按响应临时起线程建 runtime，避免异常流量把降级路径放大成资源放大器。
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
- Dropping an unresponded `IncomingRequest` now emits a JSON-RPC internal error for both direct and batch requests, including sync/no-runtime drop paths, so peers never hang waiting for a missing response and batch flushes still complete.
- Added direct-request regression coverage for the sync/no-runtime drop path so dropping a handler-owned request outside a current Tokio runtime still returns the expected JSON-RPC internal error.
- Added regression coverage for the `streamable_http` path where an already-open SSE stream must drop the stale connection, reconnect after a POST response rolls the session id, and continue delivering server notifications on the new session.
- `mcp-jsonrpc` now finishes batch-response flushes even when the last dropped request is released from a sync/no-runtime context, so sibling responses do not hang behind a leaked final flush.
