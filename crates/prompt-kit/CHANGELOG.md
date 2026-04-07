# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Fixed
- `bootstrap_prompt_directory(...)` 在 load 失败后又遇到 rollback 失败时，错误链现在优先暴露 rollback 作为主 `source()`，并把原始 load 失败挂到下一层 source，避免外层 `io::ErrorKind`、错误链和访问器各指向不同故障。

### Changed
- `LazyPromptDirectory` 的 blocking-shim 契约继续保持“并发访问等待既有初始化结果”，不再因为底层 `LazyValue` 把同线程 in-flight 状态误判成递归而提前 fail-fast；直接递归初始化仍然显式拒绝。
- `prompt-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `text-assets-kit`，发布契约收口为 Git / monorepo 复用而不是暗示可独立 crates.io 发布。
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a prompt-local `lazy_state` and duplicate bootstrap orchestration.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Promoted a runtime-owned `PromptDirectoryHandle` as the canonical shared prompt directory handle and downgraded `LazyPromptDirectory` to a deprecated blocking compatibility path.
- `LazyPromptDirectory` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `prompt-kit` 内部对 `text-assets-kit::lazy_value::*` 的 compat-shim 依赖现在也显式标注为 `#[allow(deprecated)]`，避免仓内 compat path 悄悄绕过上游的边界信号。
- `prompt-kit` 现在通过 `text-assets-kit` crate-root 的 deprecated compat re-export 使用 `LazyValue`，不再依赖已收口的 `text_assets_kit::lazy_value` 模块路径。
- `prompt-kit` 现在补齐了 `bootstrap_prompt_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续让相对 prompt 目录依赖 ambient `current_dir()`。
- `LazyPromptDirectory` 现在把检测到的线程级跨线程初始化环路收敛成显式错误，而不是把兼容层调用者永久卡死。
- `LazyPromptDirectory` 现在会把“同线程但并非当前递归调用”的初始化冲突单独映射成显式错误，避免 deprecated blocking shim 继续把这类冲突误报成 reentrant 初始化。
- `LazyPromptDirectory` 的同线程冲突错误现在直接说明这是 blocking compatibility shim 边界，并指向 `PromptDirectoryHandle` 作为 runtime-facing canonical handle，避免调用方继续把这类失败误读成普通目录加载错误。
- `PromptDirectoryHandle` 现在复用 `text-assets-kit::SharedRuntimeHandle<TextDirectory>`，不再在 crate 内部维护第二套几乎同构的 runtime handle 实现。

### Added
- Split prompt-directory bootstrap and lazy runtime handle logic out of the old mixed runtime-assets crate so prompt-specific behavior now lives behind its own domain crate.
- Added `PromptDirectoryHandle`, a hot-swappable prompt snapshot handle that serves reads without blocking on first-use initialization.
