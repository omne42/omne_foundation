# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- 明确内建 provider 的 ambient CLI 发现边界：默认只信任 ambient allowlist 中的系统目录级 `PATH` 项，并把转发给 builtin CLI 子进程的 `PATH` 同步裁剪到同一可信目录集合；工作区 shim 或用户目录二进制仍需通过显式绝对路径 override 接入。
- `secret-kit` 现在把内建 provider 的 parse/command 细节下沉到 `spec/providers.rs` 私有模块，`spec.rs` 只保留通用 `secret://` 入口、env/file 语义和共享 helper，从而把核心流程与 provider 专属 CLI 策略分层开来而不改变公开 `SecretSpec`/`secret://` 契约。
- `SecretResolver` 现在返回 boxed future 并保持 object-safe，调用方可以用 `Arc<dyn SecretResolver>` 之类的动态组装边界而不必锁死在静态泛型上；同时补了 trait-object 回归测试。
- `secret-kit`：把 secret value 容器与 command-runtime/context trait 从 `lib.rs` 拆到独立模块，收窄 crate 入口文件的职责边界；公开 API 与行为保持不变。
- `secret-kit` 现在在内建 CLI 的 ambient `PATH` 解析链路里保留已解析程序路径的 `OsString/PathBuf` 形态，不再把 non-UTF-8 可执行路径经 `to_string_lossy()` 降成文本后再交给 `Command::new(...)`，避免在非 UTF-8 目录下把 `vault`/`aws`/`gcloud`/`az` 解析到错误路径。
- `CachingSecretResolver` 现在允许 cacheable resolver 显式声明 `SecretCommandRuntime` 是否属于缓存边界；runtime-sensitive secret 会把 command runtime partition 一并纳入 cache key，缺失稳定 runtime partition 时 fail-closed 禁止复用，避免同一 environment 分区下误复用不同 CLI/runtime 上下文的 secret。
- `secret-kit` 现在把内建 ambient command runtime 也视为“不稳定 cache 边界”：ambient 路径默认不提供 runtime cache partition，因此 runtime-sensitive secret 不会在 ambient `PATH`/进程环境上下文上静默复用缓存，除非调用方显式提供稳定 runtime partition。
- Collapse the cache-hint fast-path lookup in `CachingSecretResolver` so the crate stays clean under the workspace `clippy::all` local gate after the mismatched-hint fix.
- `secret-kit` 现在会在 Linux 上于 secret command leader 退出时立刻触发 process-tree cleanup，并把 orphan cleanup 交给后台短时重试，避免慢速 `/proc` 观察窗口导致成功路径遗漏残留 helper 进程。
- `secret-kit`：Linux orphan cleanup 的后台重试窗口现在覆盖完整回归测试观测区间，避免慢速 GitHub Actions runner 上 `/proc` 进程组可见性滞后导致清理线程过早停止。
- `secret-kit`：Linux secret-command process-tree cleanup 现在把 orphan retry 收口到共享后台 worker，而不是每次成功/失败路径都 spawn 一个最长 12 秒的清理线程；保留现有 retry 语义，同时避免高吞吐下的线程放大。
- `secret-kit`：Linux orphan cleanup 回归测试现在会先确认后台 helper 已经加入 shell 的 process group，再断言成功/取消路径的清理结果，避免 GitHub Actions runner 上因 shell 时序抖动产生误判。
- `secret-kit`：Linux cleanup 回归测试现在按真实超时窗口轮询 pid 文件和进程退出，并在写出后台 pid 后短暂保留 shell leader，减少慢速 GitHub Actions runner 上的误报失败而不放宽断言语义。
- Stabilize the Linux process-group cleanup gate further by switching the helper waits from fixed iteration counts to explicit time budgets, so slower GitHub Actions runners have more room to observe pid-file writes and tree termination without weakening the assertions.
- `secret-kit`：CLI-backed secret resolution 现在会先验证 Tokio time driver，再进入命令超时控制；错误配置不再 panic，而是返回带稳定 catalog code 的命令错误，并把运行时前提写进 `SecretCommandRuntime` 文档。
- `CachingSecretResolver` 现在只把 cache hint 用于命中 fast-path；当 hint 与 prepared cache scope 不匹配时，不再错误地在坏 hint 上串行化不同 secret 的并发解析或共享 leader，而是按文档契约退化成普通 cache miss。
- `secret-kit` 的 crate-level 文档示例现在提供最小可编译上下文，`cargo test -p secret-kit --doc -- --ignored` 不再因为裸 `let` / `.await?` 片段而失败。
- Stabilize Linux process-group cleanup tests by detaching background-command stdio, tracking PID identity to avoid `/proc` reuse false negatives, extending the cleanup polling budget, and briefly keeping the shell leader alive after it records the background pid so slower CI runners can reliably capture process-tree cleanup state without changing library semantics.
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Kept `secret-kit` focused on secret-specific semantics while moving shared process-tree primitives out to the systems layer and preserving structured error texts.
- Retry Unix `ETXTBSY` (`Text file busy`) command spawns briefly so freshly materialized builtin CLI shims do not introduce flaky secret resolution failures.
- Move the Unix `ETXTBSY` spawn-retry backoff onto Tokio time so async secret resolution no longer blocks executor workers while preserving the same retry contract.
- Mark deterministic local file/input failures as `DoNotRetry` while keeping transient I/O and CLI timeout/spawn failures retryable so upstream callers stop misclassifying secret resolution incidents.
