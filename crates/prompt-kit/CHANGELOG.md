# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a prompt-local `lazy_state` and duplicate bootstrap orchestration.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Promoted a runtime-owned `PromptDirectoryHandle` as the canonical shared prompt directory handle and downgraded `LazyPromptDirectory` to a deprecated blocking compatibility path.
- `LazyPromptDirectory` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `prompt-kit` 现在补齐了 `bootstrap_prompt_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续让相对 prompt 目录依赖 ambient `current_dir()`。

### Added
- Split prompt-directory bootstrap and lazy runtime handle logic out of the old mixed runtime-assets crate so prompt-specific behavior now lives behind its own domain crate.
- Added `PromptDirectoryHandle`, a hot-swappable prompt snapshot handle that serves reads without blocking on first-use initialization.
