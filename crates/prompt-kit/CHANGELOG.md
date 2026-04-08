# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

- `prompt-kit`：当 prompt directory load 失败且后续 best-effort rollback 也失败时，返回的 `io::Error` 现在保留原始 load `ErrorKind`，并通过 `PromptBootstrapCleanupError` 同时暴露 rollback 失败；主错误语义不再被 rollback 覆盖。

### Fixed
- `bootstrap_prompt_directory(...)` 在 load 失败后又遇到 rollback 失败时，错误链和 `io::ErrorKind` 现在都继续对齐到 load failure；rollback 失败继续通过 `PromptBootstrapCleanupError` 的访问器保留，不再把主因重分类成 cleanup。
- `prompt-kit` 的资源 bootstrap 回归测试现在会先探测 `OMNE_TEST_SHORT_TMPDIR`、环境临时根和 Unix `/var/tmp` fallback 是否可用；tempdir 创建本身失败或 `StorageFull` 等非业务性临时目录故障时都会显式跳过，避免受限 runner 因磁盘/临时目录条件误报失败。

### Changed
- `LazyPromptDirectory` 的 blocking-shim 契约继续保持“其他线程上的并发访问等待既有初始化结果”；直接递归初始化、同线程 in-flight 初始化冲突和可检测的线程级跨线程环路都继续显式报错，避免把这类可诊断冲突退化成阻塞。
- `LazyPromptDirectory` 现在改为复用 `prompt-kit` 自己的私有 blocking compat shim；`text-assets-kit` 不再向外暴露通用 `LazyValue` / `LazyInitError` surface，prompt 目录兼容层也不再把这条跨域 public API 当作 foundation 复用面。
- `prompt-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `text-assets-kit`，发布契约收口为 Git / monorepo 复用而不是暗示可独立 crates.io 发布。
- Reused `text-assets-kit` bootstrap+rollback primitives while keeping `LazyPromptDirectory` 的阻塞 compat shim 收口在 prompt 域本地，而不是继续依赖跨 crate 的通用 lazy-init public surface.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Promoted a runtime-owned `PromptDirectoryHandle` as the canonical shared prompt directory handle and downgraded `LazyPromptDirectory` to a deprecated blocking compatibility path.
- `LazyPromptDirectory` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `prompt-kit` 现在补齐了 `bootstrap_prompt_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续让相对 prompt 目录依赖 ambient `current_dir()`。
- `LazyPromptDirectory` 现在把检测到的线程级跨线程初始化环路收敛成显式错误，而不是把兼容层调用者永久卡死。
- `LazyPromptDirectory` 现在会把“同线程但并非当前递归调用”的初始化冲突单独映射成显式错误，避免 deprecated blocking shim 继续把这类冲突误报成 reentrant 初始化。
- `LazyPromptDirectory` 的同线程冲突错误现在直接说明这是 blocking compatibility shim 边界，并指向 `PromptDirectoryHandle` 作为 runtime-facing canonical handle，避免调用方继续把这类失败误读成普通目录加载错误。
- `PromptDirectoryHandle` 现在复用 `text-assets-kit::SharedRuntimeHandle<TextDirectory>`，不再在 crate 内部维护第二套几乎同构的 runtime handle 实现。
- `prompt-kit::prompts` 不再作为 public implementation module 暴露；crate root 继续提供稳定的 prompt runtime adapter 入口，并移除实现模块路径这条可误用的兼容面。

### Added
- Split prompt-directory bootstrap and lazy runtime handle logic out of the old mixed runtime-assets crate so prompt-specific behavior now lives behind its own domain crate.
- Added `PromptDirectoryHandle`, a hot-swappable prompt snapshot handle that serves reads without blocking on first-use initialization.
