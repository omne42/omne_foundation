# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

- `i18n-runtime-kit`：当 manifest bootstrap 之后的 catalog load 失败且 best-effort rollback 也失败时，错误现在显式归类为 `ResourceCatalogError::LoadRollback(...)`，保留原始 load/rollback 双错误，而不再把这类双失败误重分类成 bootstrap 主错误。
- `i18n-runtime-kit`：`ResourceCatalogError::LoadRollback(...)` 现在对 cleanup payload 做 `Box` 封装，保持双错误语义不变，同时继续满足 workspace 的 `clippy::result_large_err` 质量门禁。

### Changed
- `LazyCatalog` 的 blocking-shim 契约继续保持“其他线程上的并发访问等待既有初始化结果”；直接递归初始化、同线程 in-flight 初始化冲突和可检测的线程级跨线程环路都继续显式报错，避免把这类可诊断冲突退化成阻塞。
- `LazyCatalog` 现在改为复用 `i18n-runtime-kit` 自己的私有 blocking compat shim；`text-assets-kit` 不再向外暴露通用 `LazyValue` / `LazyInitError` surface，runtime i18n 兼容层也不再把这条跨域 public API 当作 foundation 复用面。
- `i18n-runtime-kit::lazy_catalog` 不再作为 public implementation module 暴露；crate root 继续提供稳定的 runtime adapter 入口，并移除实现模块路径这条可误用的兼容面。
- `i18n-runtime-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `text-assets-kit`，发布契约收口为 Git / monorepo 复用而不是暗示可独立 crates.io 发布。
- Reused `text-assets-kit` bootstrap+rollback primitives while keeping `LazyCatalog` 的阻塞 compat shim 收口在 i18n 域本地，而不是继续依赖跨 crate 的通用 lazy-init public surface.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Clarified `GlobalCatalog` as the runtime-facing canonical handle and downgraded the root `LazyCatalog` export to a deprecated blocking compatibility path.
- `LazyCatalog` 本体现在也带 `#[deprecated]` 标记，和 README / crate root 的兼容层定位保持一致，避免 runtime-facing 调用方继续把它误当成推荐入口。
- `i18n-runtime-kit` 现在补齐了显式 base 驱动的公共入口：`bootstrap_i18n_catalog_with_base(...)`、`load_i18n_catalog_from_directory_with_base(...)`、`reload_i18n_catalog_from_directory_with_base(...)`，让调用方在已知 workspace/root 时不必继续依赖 ambient `current_dir()`。
- `LazyCatalog` 现在把检测到的线程级跨线程初始化环路收敛成显式错误，而不是把兼容层调用者永久卡死。
- `LazyCatalog` 现在会把“同线程但并非当前递归调用”的初始化冲突单独映射成显式错误，避免 deprecated blocking shim 继续把这类冲突误报成 reentrant 初始化。
- `LazyCatalog` 的同线程冲突错误现在直接说明这是 blocking compatibility shim 边界，并指向 `GlobalCatalog` 作为 runtime-facing canonical handle，避免调用方继续把这类失败误读成普通业务初始化错误。
- `GlobalCatalog` 现在复用 `text-assets-kit::SharedRuntimeHandle<dyn Catalog>`，不再在 crate 内部维护第二套几乎同构的 runtime handle 实现。

### Fixed
- Kept the unix socket entry regression test under a short non-symlink temp root so pre-commit and CI still exercise directory validation instead of failing on host socket path-length limits.
- `i18n-runtime-kit`：Unix socket 目录回归测试不再硬编码 `/var/tmp`；现在使用 `tempfile` 选择的环境临时根，并在 socket setup 本身不可用时显式跳过，避免受限 runner 因非业务性临时目录或磁盘问题误报失败。
- `i18n-runtime-kit`：Unix socket 目录回归测试现在也支持 `OMNE_TEST_SHORT_TMPDIR`，让 harness 能在 `cargo test` 重写 `TMPDIR` 到长路径时显式提供一个更短的可写根。
- `i18n-runtime-kit`：managed resource bootstrap 回归测试现在也会在 `OMNE_TEST_SHORT_TMPDIR`、`/var/tmp` 和环境临时根之间探测可写目录；如果宿主临时盘不可用，会显式跳过而不是在创建测试根时直接 panic。
- `i18n-runtime-kit`：当 runner 没显式设置 `TMPDIR` 且默认临时根仍是 `/tmp` 时，测试辅助代码现在会优先尝试环境临时根，再回退到 `/var/tmp`；不会再把 `/var/tmp` 当作默认首选根去掩盖 harness 已经提供的临时目录约束。
- Removed a redundant borrow in the explicit-base catalog directory scan path so the crate continues to pass the workspace `clippy -D warnings` gate.

### Added
- Split runtime i18n asset loading, bootstrap, and lazy/global catalog handles out of the old mixed runtime-assets crate so the i18n domain now owns its own runtime adapter boundary.
- Added runtime-owned CLI / argv locale parsing plus `CliLocaleError`, so command-line locale input no longer leaks back into `i18n-kit`.
- Added a regression test for `reload_i18n_catalog_from_directory_with_base(...)` so the explicit-base boundary remains covered when process `current_dir()` drifts.
