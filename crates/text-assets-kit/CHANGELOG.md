# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Added shared `LazyValue` / `LazyInitError` primitives plus `bootstrap_text_resources_then_load(...)`, so domain runtime adapters can reuse the same lazy-init and bootstrap+rollback orchestration without reimplementing it per crate.
- Added explicit-base variants for path/data-root resolution: `materialize_resource_root_with_base(...)`, `resolve_data_root_with_base(...)`, and `ensure_data_root_with_base(...)`. Callers that already know their workspace root no longer need to rely on ambient `current_dir()` to resolve relative text-assets paths.
- Added explicit-base variants for higher-level text asset entry points: `TextDirectory::load_with_base(...)`, `TextDirectory::load_resource_files_with_base(...)`, `bootstrap_text_resources_then_load_with_base(...)`, `bootstrap_text_resources_with_base(...)`, `bootstrap_text_resources_with_report_with_base(...)`, and `scan_text_directory_with_base(...)`.
- Added `SharedRuntimeHandle<T>`, a narrow hot-swappable runtime snapshot handle that higher-level domain adapters can reuse instead of each carrying their own `RwLock<Option<Arc<_>>>` implementation.
- Added `LazyInitConflictKind` plus `LazyInitError::conflict_kind()`, so compat-shim callers can distinguish stable blocking/conflict causes without relying on display-string matching.

### Fixed
- `LazyValue` 现在只把当前调用栈上的真实重入视为 `ReentrantInitialization`；如果已有初始化恰好记录在同一 OS 线程上，后续访问会像其他 waiters 一样等待既有结果，不再被误判成 `SameThreadInitializationConflict`。
- `LazyValue` no longer tears down its cross-thread wait edge on every `Condvar` wake-up; spurious or unrelated `notify_all()` calls now keep the tracked wait relationship alive until the in-flight attempt actually settles, so cycle detection does not silently lose visibility mid-wait.
- `text-assets-kit` no longer vendors `omne-fs-primitives` inside `omne_foundation`; it now resolves filesystem primitives from the canonical `omne-runtime` crate instead of maintaining a second workspace-local copy.
- `text-assets-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `omne-fs-primitives`，不再让 manifest 暗示可单独 crates.io 发布。
- `text-assets-kit` 的 Unix bootstrap lock 目录在缺少 `XDG_RUNTIME_DIR` 和 `/run/user/<uid>` 时，现会继续尊重进程级临时目录选择（例如 `TMPDIR`）而不是硬编码 `/tmp`；这样 workspace checks 和依赖它的 runtime bootstrap 在“根盘满但另一个挂载点可用”的环境里不再被错误临时目录卡死。
- `LazyValue` 现在把“同线程但并非当前递归调用”的 in-flight 初始化冲突单独建模出来，不再把这类 blocking compatibility shim 的死锁前兆误报成 `ReentrantInitialization`。
- `LazyValue` now detects thread-level cross-thread wait cycles between tracked lazy initializers and fails fast with an explicit error instead of leaving both threads permanently blocked.
- Cleaned up the new `LazyWaitGraph` wait-state teardown branch so the crate continues to satisfy the workspace `clippy -D warnings` CI gate after the cycle-detection fix.
- `lock_bootstrap_transaction(...)` now fails closed when inspecting or canonicalizing the bootstrap root prefix fails, instead of silently deriving an unstable lock key from partial path information.
- `text-assets-kit`：Unix 下的 bootstrap advisory lock 目录不再固定落在全局 `/tmp/.text-assets-kit-bootstrap-locks`；现在优先使用用户级 runtime 目录（`$XDG_RUNTIME_DIR` 或 `/run/user/<uid>`），缺失时退回 `/tmp/.text-assets-kit-bootstrap-locks/uid-<uid>` 的每用户命名空间，避免跨用户共享同一全局锁目录。
- Kept the unix socket entry regression test under a short non-symlink temp root so hook and CI runs do not fail on host `sun_path` limits before the real validation path executes.
- `text-assets-kit`：默认 `DataRootScope::Auto` 不再在 workspace-local root 缺失时静默退回 `$HOME/.text_assets`；现在会继续使用 `<cwd>/.text_assets` 并按需创建，避免把本应局部的状态悄悄放大成 user-global 边界。
- `text-assets-kit`：收紧 `DataRootScope::Auto` 后同步清理实现中的多余 `return`，确保 crate 继续满足 workspace `clippy -D warnings` 质量门禁。
- Cleaned up the new explicit-base secure-root loading paths to satisfy the workspace `clippy::needless_borrow` gate without changing runtime behavior.

### Changed
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Renamed the old mixed `runtime-assets-kit` boundary to `text-assets-kit` and narrowed it to generic text-resource path validation, secure filesystem access, data-root helpers, and bootstrap/rollback primitives.
- Kept the shared text-manifest bootstrap path public so downstream domain adapters can reuse it without reaching into private modules.
- Clarified that bootstrap/rollback only serializes same-root attempts and performs best-effort cleanup for the current attempt; it does not promise crash-safe or power-loss-recovery transactions.
- Demoted the ambient `materialize_resource_root(...)`, `resolve_data_root(...)`, and `ensure_data_root(...)` helpers to compatibility-only entry points, and routed crate-internal callers through private compat helpers so explicit-base APIs remain the canonical workspace-boundary surface.
- Demoted `BootstrapTransactionGuard` / `lock_bootstrap_transaction(...)` from crate-root first-class exports to deprecated compatibility re-exports, and documented `bootstrap_lock` as a hidden low-level module instead of a canonical boundary entry.
- Demoted the root `LazyValue` / `LazyInitError` exports to deprecated compatibility re-exports and documented the underlying lazy module as a blocking shim instead of an async runtime-facing foundation API.
- `text-assets-kit::lazy_value` 不再作为可直接下钻的 public module 暴露；blocking compat shim 现在只保留 crate-root 的 deprecated re-export，进一步收窄 runtime-facing 公共面。
- `LazyValue` / `LazyInitError` 的类型定义本体现在也带 `#[deprecated]` 标记；crate-root 兼容入口和类型定义本体都会持续暴露 compat-shim 边界信号。
- Documented the ambient `current_dir()` resolution helpers as compatibility entry points; explicit-base APIs are now the canonical boundary whenever the caller already owns a stable workspace root.
