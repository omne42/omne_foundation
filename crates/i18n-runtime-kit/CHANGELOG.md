# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `i18n-runtime-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `text-assets-kit`，发布契约收口为 Git / monorepo 复用而不是暗示可独立 crates.io 发布。
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a second copy inside `i18n-runtime-kit`.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Clarified `GlobalCatalog` as the runtime-facing canonical handle and downgraded the root `LazyCatalog` export to a deprecated blocking compatibility path.
- `LazyCatalog` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `i18n-runtime-kit` 现在补齐了显式 base 驱动的公共入口：`bootstrap_i18n_catalog_with_base(...)`、`load_i18n_catalog_from_directory_with_base(...)`、`reload_i18n_catalog_from_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续依赖 ambient `current_dir()`。
- `LazyCatalog` 现在把检测到的线程级跨线程初始化环路收敛成显式错误，而不是把兼容层调用者永久卡死。

### Fixed
- Kept the unix socket entry regression test under a short non-symlink temp root so pre-commit and CI still exercise directory validation instead of failing on host socket path-length limits.
- Removed a redundant borrow in the explicit-base catalog directory scan path so the crate continues to pass the workspace `clippy -D warnings` gate.

### Added
- Split runtime i18n asset loading, bootstrap, and lazy/global catalog handles out of the old mixed runtime-assets crate so the i18n domain now owns its own runtime adapter boundary.
- Added runtime-owned CLI / argv locale parsing plus `CliLocaleError`, so command-line locale input no longer leaks back into `i18n-kit`.
