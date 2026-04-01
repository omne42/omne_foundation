# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `prompt-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `text-assets-kit`，发布契约收口为 Git / monorepo 复用而不是暗示可独立 crates.io 发布。
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a prompt-local `lazy_state` and duplicate bootstrap orchestration.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Promoted a runtime-owned `PromptDirectoryHandle` as the canonical shared prompt directory handle and downgraded `LazyPromptDirectory` to a deprecated blocking compatibility path.
- `LazyPromptDirectory` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `prompt-kit` 现在补齐了 `bootstrap_prompt_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续让相对 prompt 目录依赖 ambient `current_dir()`。
- `LazyPromptDirectory` 现在把检测到的线程级跨线程初始化环路收敛成显式错误，而不是把兼容层调用者永久卡死。
- `LazyPromptDirectory` 现在会把“同线程但并非当前递归调用”的初始化冲突单独映射成显式错误，避免 deprecated blocking shim 继续把这类冲突误报成 reentrant 初始化。

### Added
- Split prompt-directory bootstrap and lazy runtime handle logic out of the old mixed runtime-assets crate so prompt-specific behavior now lives behind its own domain crate.
- Added `PromptDirectoryHandle`, a hot-swappable prompt snapshot handle that serves reads without blocking on first-use initialization.
