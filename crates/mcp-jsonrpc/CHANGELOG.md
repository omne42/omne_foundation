# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `mcp-jsonrpc` manifest 现在为内部 path 依赖补上显式 version 约束，并显式标记 `publish = false`；在 runtime primitives 形成独立发布链之前，crate 不再把 Git/monorepo 复用边界伪装成 crates.io 可直接发布。
- `mcp-jsonrpc`：`IncomingRequest` 现在用显式 owner 计数而不是 `Arc::strong_count()` 判断“最后一个未响应 clone 被 drop”，并补上并发 drop 回归测试；handler clone 并发释放时不再因为竞态漏发自动 `internal error` 响应。
- `mcp-jsonrpc`：no-runtime detached fallback 现在显式保留“每任务独立 fallback thread + runtime”的隔离语义，并补上 fallback 线程创建失败注入；`schedule_close_once(...)` 与 dropped-request 回归测试现在覆盖“helper/thread 起不来时 fail-closed 但不 panic”以及“一个阻塞 fallback 任务不会把后续 close/response/batch flush 串行拖死”。
- `mcp-jsonrpc`：`stdout_log` 的 capability-style open 现在按平台条件导入 `OpenOptionsExt`；Unix 继续保留 `mode(0o600)` 权限收敛，Windows 不再因未使用 import 被 `-D warnings` 门禁拦下。
- `mcp-jsonrpc`：`stdout_log_creates_missing_parent_dirs` 回归测试现在在读取文件前显式 flush；断言不再依赖 Tokio 文件句柄 drop 时机，避免 Linux CI 上出现假阴性。
- `mcp-jsonrpc`：stdout log 现在通过 capability-style no-follow open 同步创建缺失父目录，而不是在 symlink 预检后再调用 ambient `create_dir_all`；缺失父目录场景不再暴露“检查后被竞态替换”的 TOCTOU 窗口。
- `mcp-jsonrpc`：`detached_spawners_do_not_share_process_global_worker` 回归测试不再把 shared-worker spawn 次数钉死为恰好 `2`；现在校验“至少新起了两个 worker”，避免 worker 重建路径把实现细节误判成 CI flake。
- `mcp-jsonrpc`：detached cleanup/runtime 不再复用 crate 级隐藏单例 worker；后台补偿任务现在由每个 client lifecycle 自己持有和调度，`ClientHandle` 无生命周期时才退化到单任务 fallback runtime，避免跨 client 偷偷共享全进程 runtime/thread。
- `mcp-jsonrpc`：detached runtime 回归测试现在显式创建独立 spawner，并新增“两个 spawner 会各自起 worker”覆盖，防止后续重构重新把生命周期收口成进程级全局状态。
- `mcp-jsonrpc`：无 Tokio runtime 的 batch flush 补偿现在和 dropped-response 补偿一样带有明确超时；坏写端不会把后台 flush 任务无限挂住，超时后会 fail closed 关闭 transport。
- `mcp-jsonrpc`：batch 收尾现在会把 `finish()` 写失败显式回传给入站主状态机，避免批量 invalid-request/error response 的 flush 失败继续被静默吞掉；同时 reserved response 在已关闭 transport 上也会归还 pending slot，不再留下脏 completion 计数。
- `mcp-jsonrpc`：detached runtime 的 shared worker 现在只有在任务真正被 runtime 接手后才把调度视为成功；worker 若在启动任务前退出，调用点会回退到单任务 fallback runtime，而不是把已接收但未执行的 cleanup silently drop。
- `mcp-jsonrpc`：`Client::close_in_background_once(...)` 现在和显式 close 一样会同时 abort reader task 与 transport tasks；best-effort 后台关闭不再只关写端留下悬挂读循环或 SSE/POST transport 任务。
- `mcp-jsonrpc`：`ClientHandle::close(...)`、内部 `close_with_reason(...)` 和 timeout/写失败触发的 `schedule_close_once(...)` 现在也会复用同一套 reader/transport lifecycle 收尾；持有 handle 的调用方不再只关闭写端而把 reader 或 streamable-http transport 留在后台悬挂。
- `mcp-jsonrpc`：`ClientHandle` 关闭路径现在在同一临界区内记录首个 `close_reason` 并发布 `closed`，并且无 runtime 的最后兜底 close 收尾会等待 busy writer 释放后再替换写端；`is_closed()`/`check_closed()` 不再暴露“已关闭但原因已被其他竞态路径抢写”或“写端正忙时直接放弃收尾”的窗口。
- `mcp-jsonrpc`：batch response flush 的完成判定改为单原子状态机，消除了 `finish()` 与最后一个异步响应并发时双方都早退、整批响应永远不 flush 的竞态。
- `mcp-jsonrpc`：对超时关闭诊断的内部格式化写法做了风格收敛，保持与 workspace 的 Clippy 门禁一致而不改变运行时行为。
- `mcp-jsonrpc`：无 Tokio runtime 的 detached cleanup 调度现在在 shared worker 不可用时退化到“每任务独立 fallback thread + runtime”，不再 inline `block_on` 调用方；shared worker 与 fallback runtime 都不可用时仍会把失败显式回传给上层 fail-closed，而不是 panic 或静默丢任务。
- `mcp-jsonrpc`：共享 detached runtime worker 现在会把收到的 cleanup 任务并发 `spawn` 到同一个 Tokio runtime 上执行，而不是按队列串行 `block_on`；单个卡住的 dropped-request / batch-flush cleanup 不会再把后续 cleanup 全部拖死。
- `mcp-jsonrpc`：close 路径现在先同步标记 `closed` 并 drain pending request，再异步收尾写端；`wait_with_timeout` 即使在 close-stage 卡在写锁上超时，也不会再留下“新请求已拒绝、旧 pending 还悬挂”的半关闭窗口。
- `mcp-jsonrpc`：当本地 server-request handler 已关闭或已满时，reader 不再直接在主循环里等待错误回包写完；错误响应改为有界后台补偿，写不出去会 fail closed，避免坏写端把 reader 主状态机一起拖死。
- `mcp-jsonrpc`：多行 SSE `data:` event 现在要求整个 event payload 本身必须是合法 JSON；非 JSON 的多行 event 会 fail closed，不再被桥接成多条伪 JSON-RPC line 污染下游状态机。
- `mcp-jsonrpc`：无 Tokio runtime 的 detached task 调度现在把 shared worker 初始化失败和 fallback runtime 失败都显式回传给调用点；补偿任务无法调度时会关闭 transport，而不是继续静默丢掉 dropped-request / batch-flush 后续动作。
- `mcp-jsonrpc`：detached runtime 相关单测现在复用同一把测试互斥并在 no-runtime drop 用例前后显式 reset worker 状态，避免并行单测彼此污染共享 worker/failure-injection 全局态。
- `mcp-jsonrpc`：`StreamableHttpOptions.headers` 现在会 fail-fast 拒绝 transport-owned 的 `mcp-session-id`，直接调用 transport 也不能预先伪造或固定会话头。
- `mcp-jsonrpc`：将 `Error` / `ProtocolError*` 与 error-record 映射、detached runtime 后台 worker，以及 crate-local `#[cfg(test)]` 模块分别拆到独立源码文件，显著收窄 `src/lib.rs` 的职责边界而不改变公开 API。
- `mcp-jsonrpc`：detached runtime 的共享后台 worker 现在在任务 panic 或 worker 初始化失败后会显式重建；如果共享 worker 仍不可用，会退化到单任务 fallback runtime，而不是继续把后续 dropped-request / batch-flush 补偿任务静默丢弃。
- `mcp-jsonrpc`：出站 transport 写入一旦返回 `io::Error`，`ClientHandle` 现在会立刻 fail-closed 并记录首个 close reason；dropped-request 的后台补响应也改成有界等待，写失败或超时时不再静默吞掉，而会关闭连接避免对端无限悬挂。
- `Limits::max_message_bytes` 现在重新同时约束出站 request / notification / response 的序列化大小；超限帧会在写入前 fail-fast 返回稳定错误，避免配置名和实际行为继续脱节。
- `mcp-jsonrpc`：`ClientHandle::close_reason()` 的文档现在明确它只暴露 first-writer best-effort close diagnostics；并发关闭路径里哪一个 source 先写入并不构成稳定契约。
- `mcp-jsonrpc`：`StreamableHttpOptions.proxy_mode` 现在真正贯通到底层 HTTP client；显式选择 `UseSystem` 时会读取系统代理环境，只有 `enforce_public_ip` 的 pinned socket 路径仍会继续禁用代理。
- `mcp-jsonrpc`：`streamable_http` 的独立 SSE 读侧现在会在正常 EOF 后自动重连，而不是把整个 transport 直接关闭；会 idle-close/轮换 SSE 的服务端不会再把客户端无谓打死。
- `mcp-jsonrpc`：`streamable_http` 的 SSE 唤醒信号改为无丢失传递，`SessionChanged` 不会再被排队中的 `Connect` 挤掉，活跃 SSE 在 session rollover 后会可靠切到新会话。
- `mcp-jsonrpc`：入站 server notification 在本地通知队列过载或接收端已关闭时不再静默丢弃；transport 现在会记录 stats 并主动关闭连接，把数据丢失显式暴露给调用方。
- `mcp-jsonrpc`：reader 现在对非法 JSON 和非法 JSON-RPC 帧 fail-closed；在记录诊断/返回 `invalid request` 后会立即关闭连接并清空 pending request，避免把协议损坏伪装成后续超时。
- `mcp-jsonrpc`：同步/无 Tokio runtime 的 dropped-request 与 batch flush 补偿路径现在复用单例后台 runtime，而不是按响应临时起线程建 runtime，避免异常流量把降级路径放大成资源放大器。
- `mcp-jsonrpc`：当 detached shared worker 或 fallback thread 本身创建失败时，background runtime 现在会退化到当前线程 best-effort 执行或静默放弃补偿任务，而不是直接 panic；对应回归测试覆盖了“双重 spawn 失败也不崩”的路径。
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
- Stabilized the streamable-HTTP SSE reconnect regression coverage by forcing the initial SSE responses in both graceful-EOF and session-rollover tests to advertise `Connection: close`, alongside a less timing-sensitive wait window on slower CI runners.
