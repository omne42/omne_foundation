# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Collapse the cache-hint fast-path lookup in `CachingSecretResolver` so the crate stays clean under the workspace `clippy::all` local gate after the mismatched-hint fix.
- `secret-kit` 现在会在 Linux 上于 secret command leader 退出时立刻触发 process-tree cleanup，并把 orphan cleanup 交给后台短时重试，避免慢速 `/proc` 观察窗口导致成功路径遗漏残留 helper 进程。
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
