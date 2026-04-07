# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `LazyCatalog` 的 blocking-shim 契约继续保持“并发访问等待既有初始化结果”，不再因为 `LazyValue` 把同线程 in-flight 状态误判成递归而提前 fail-fast；直接递归初始化仍然显式拒绝。
- `i18n-runtime-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `text-assets-kit`，发布契约收口为 Git / monorepo 复用而不是暗示可独立 crates.io 发布。
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a second copy inside `i18n-runtime-kit`.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Clarified `GlobalCatalog` as the runtime-facing canonical handle and downgraded the root `LazyCatalog` export to a deprecated blocking compatibility path.
- `LazyCatalog` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `i18n-runtime-kit` 内部对 `text-assets-kit::lazy_value::*` 的 compat-shim 依赖现在也显式标注为 `#[allow(deprecated)]`，避免仓内 compat path 悄悄绕过上游的边界信号。
- `i18n-runtime-kit` 现在通过 `text-assets-kit` crate-root 的 deprecated compat re-export 使用 `LazyValue`，不再依赖已收口的 `text_assets_kit::lazy_value` 模块路径。
- `i18n-runtime-kit` 现在补齐了显式 base 驱动的公共入口：`bootstrap_i18n_catalog_with_base(...)`、`load_i18n_catalog_from_directory_with_base(...)`、`reload_i18n_catalog_from_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续依赖 ambient `current_dir()`。
- `LazyCatalog` 现在把检测到的线程级跨线程初始化环路收敛成显式错误，而不是把兼容层调用者永久卡死。
- `LazyCatalog` 现在会把“同线程但并非当前递归调用”的初始化冲突单独映射成显式错误，避免 deprecated blocking shim 继续把这类冲突误报成 reentrant 初始化。
- `LazyCatalog` 的同线程冲突错误现在直接说明这是 blocking compatibility shim 边界，并指向 `GlobalCatalog` 作为 runtime-facing canonical handle，避免调用方继续把这类失败误读成普通业务初始化错误。
- `GlobalCatalog` 现在复用 `text-assets-kit::SharedRuntimeHandle<dyn Catalog>`，不再在 crate 内部维护第二套几乎同构的 runtime handle 实现。

### Fixed
- Kept the unix socket entry regression test under a short non-symlink temp root so pre-commit and CI still exercise directory validation instead of failing on host socket path-length limits.
- Removed a redundant borrow in the explicit-base catalog directory scan path so the crate continues to pass the workspace `clippy -D warnings` gate.

### Added
- Split runtime i18n asset loading, bootstrap, and lazy/global catalog handles out of the old mixed runtime-assets crate so the i18n domain now owns its own runtime adapter boundary.
- Added runtime-owned CLI / argv locale parsing plus `CliLocaleError`, so command-line locale input no longer leaks back into `i18n-kit`.
- Added a regression test for `reload_i18n_catalog_from_directory_with_base(...)` so the explicit-base boundary remains covered when process `current_dir()` drifts.
