# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

- `text-assets-kit`：deprecated compat 入口和 `lazy_value` shim 实现现在改用带理由的 lint expectation，替代无说明的宽泛 `allow(deprecated)`，兼容面不变但治理点更可审计。

- `text-assets-kit`：低层 `bootstrap_lock` 实现模块不再公开暴露；crate root 对 `BootstrapTransactionGuard` / `lock_bootstrap_transaction(...)` 只保留 hidden deprecated compat 入口，文档继续收口到高层 bootstrap helpers。

### Added
- Added shared `LazyValue` / `LazyInitError` primitives plus `bootstrap_text_resources_then_load(...)`, so domain runtime adapters can reuse the same lazy-init and bootstrap+rollback orchestration without reimplementing it per crate.
- Added explicit-base variants for path/data-root resolution: `materialize_resource_root_with_base(...)`, `resolve_data_root_with_base(...)`, and `ensure_data_root_with_base(...)`. Callers that already know their workspace root no longer need to rely on ambient `current_dir()` to resolve relative text-assets paths.
- Added explicit-base variants for higher-level text asset entry points: `TextDirectory::load_with_base(...)`, `TextDirectory::load_resource_files_with_base(...)`, `bootstrap_text_resources_then_load_with_base(...)`, `bootstrap_text_resources_with_base(...)`, `bootstrap_text_resources_with_report_with_base(...)`, and `scan_text_directory_with_base(...)`.
- Added `SharedRuntimeHandle<T>`, a narrow hot-swappable runtime snapshot handle that higher-level domain adapters can reuse instead of each carrying their own `RwLock<Option<Arc<_>>>` implementation.
- Added `LazyInitConflictKind` plus `LazyInitError::conflict_kind()`, so compat-shim callers can distinguish stable blocking/conflict causes without relying on display-string matching.

### Fixed
- `BootstrapLoadError::Rollback` 现在继续通过 display 和访问器保留原始 load failure，同时把标准错误链的 `source()` 指向 rollback failure；调用方不再需要先做 enum/downcast 才能看见 cleanup 失败原因。
- `resolve_data_root_with_base(...)` / `ensure_data_root_with_base(...)` 现在会把相对 `data_dir` 和相对 `TEXT_ASSETS_DIR` override 先锚到显式 base，而不是继续要求调用方退回 ambient `current_dir()` / `HOME` 语义才能落到稳定路径。
- `LazyValue` 现在只把当前调用栈上的真实重入视为 `ReentrantInitialization`；如果已有初始化恰好记录在同一 OS 线程上，会返回显式的 `SameThreadInitializationConflict`，避免 blocking compatibility shim 把这类可诊断冲突退化成死锁。
- `LazyValue` no longer tears down its cross-thread wait edge on every `Condvar` wake-up; spurious or unrelated `notify_all()` calls now keep the tracked wait relationship alive until the in-flight attempt actually settles, so cycle detection does not silently lose visibility mid-wait.
- `text-assets-kit` no longer vendors `omne-fs-primitives` inside `omne_foundation`; it now resolves filesystem primitives from the canonical `omne-runtime` crate instead of maintaining a second workspace-local copy.
- `text-assets-kit` 现在显式标记 `publish = false`，因为它当前直接依赖 workspace-only 的 `omne-fs-primitives`，不再让 manifest 暗示可单独 crates.io 发布。
- `LazyValue` 现在把“同线程但并非当前递归调用”的 in-flight 初始化冲突单独建模出来，不再把这类 blocking compatibility shim 的死锁前兆误报成 `ReentrantInitialization`。
- `LazyValue` now detects thread-level cross-thread wait cycles between tracked lazy initializers and fails fast with an explicit error instead of leaving both threads permanently blocked.
- Cleaned up the new `LazyWaitGraph` wait-state teardown branch so the crate continues to satisfy the workspace `clippy -D warnings` CI gate after the cycle-detection fix.
- `lock_bootstrap_transaction(...)` now fails closed when inspecting or canonicalizing the bootstrap root prefix fails, instead of silently deriving an unstable lock key from partial path information.
- Kept the unix socket entry regression test under a short non-symlink temp root so hook and CI runs do not fail on host `sun_path` limits before the real validation path executes.
- `text-assets-kit`：Unix socket 目录回归测试不再硬编码 `/var/tmp`；现在使用 `tempfile` 选择的环境临时根，并在 socket setup 本身不可用时显式跳过，避免受限 runner 因非业务性临时目录或磁盘问题误报失败。
- `text-assets-kit`：Unix socket 目录回归测试现在也支持 `OMNE_TEST_SHORT_TMPDIR`，让 harness 能在 `cargo test` 重写 `TMPDIR` 到长路径时显式提供一个更短的可写根。
- `text-assets-kit` 的 bootstrap lock / managed bootstrap 回归测试现在会在临时根或锁目录出现 `StorageFull` 时显式跳过，避免受限 runner 因非业务性磁盘条件误报失败。
- `text-assets-kit`：managed bootstrap 回归测试现在也会在 `OMNE_TEST_SHORT_TMPDIR`、`/var/tmp` 和环境临时根之间探测可写目录；如果宿主临时盘不可用，会显式跳过而不是在创建测试根时直接 panic。
- `text-assets-kit`：bootstrap advisory lock 现在优先复用该资源根已存在的同盘锁命名空间；首次创建时则落到最近可写的同盘祖先，而不是抬到 `/var` 这类过高系统路径。这样在资源根从缺失到 materialize 的过程中仍保持同一跨进程锁命名空间，也避免系统 runtime/temp 盘满或系统目录不可写时误伤另一块资源盘。
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
- Reintroduced hidden deprecated crate-root `LazyValue` / `LazyInitError` compat exports so downstream deprecated shims can reuse one blocking lazy-init implementation instead of vendoring near-identical copies.
- Documented the ambient `current_dir()` resolution helpers as compatibility entry points; explicit-base APIs are now the canonical boundary whenever the caller already owns a stable workspace root.
