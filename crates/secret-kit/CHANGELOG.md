# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `secret-kit`：Linux cleanup 回归测试现在按真实超时窗口轮询 pid 文件和进程退出，并在写出后台 pid 后短暂保留 shell leader，减少慢速 GitHub Actions runner 上的误报失败而不放宽断言语义。
- `secret-kit`：Linux cleanup 回归测试现在会先记录 shell 当时真实的 process group，并确认后台进程已经加入该组，再让 leader 退出，进一步减少 GitHub Actions runner 上的误报失败而不削弱清理断言。
- `secret-kit` 现在在 secret command leader 已退出时立刻触发 process-tree cleanup，而不再把 orphaned background process 的清理延后到尾部 `Drop` 路径；这样 stdout/stderr 已经读完的成功/取消路径也能稳定回收残留子进程。
- Stabilize the Linux process-group cleanup gate further by switching the helper waits from fixed iteration counts to explicit time budgets, so slower GitHub Actions runners have more room to observe pid-file writes and tree termination without weakening the assertions.
- `secret-kit`：CLI-backed secret resolution 现在会先验证 Tokio time driver，再进入命令超时控制；错误配置不再 panic，而是返回带稳定 catalog code 的命令错误，并把运行时前提写进 `SecretCommandRuntime` 文档。
- `CachingSecretResolver` 现在只把 cache hint 用于命中 fast-path；当 hint 与 prepared cache scope 不匹配时，不再错误地在坏 hint 上串行化不同 secret 的并发解析，而是按文档契约退化成普通 cache miss。
- `secret-kit` 的 crate-level 文档示例现在提供最小可编译上下文，`cargo test -p secret-kit --doc -- --ignored` 不再因为裸 `let` / `.await?` 片段而失败。
- Stabilize Linux process-group cleanup tests by detaching background-command stdio, tracking PID identity to avoid `/proc` reuse false negatives, extending the cleanup polling budget, and briefly keeping the shell leader alive after it records the background pid so slower CI runners can reliably capture process-tree cleanup state without changing library semantics.
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Kept `secret-kit` focused on secret-specific semantics while moving shared process-tree primitives out to the systems layer and preserving structured error texts.
- Retry Unix `ETXTBSY` (`Text file busy`) command spawns briefly so freshly materialized builtin CLI shims do not introduce flaky secret resolution failures.
- Move the Unix `ETXTBSY` spawn-retry backoff onto Tokio time so async secret resolution no longer blocks executor workers while preserving the same retry contract.
- Mark deterministic local file/input failures as `DoNotRetry` while keeping transient I/O and CLI timeout/spawn failures retryable so upstream callers stop misclassifying secret resolution incidents.
