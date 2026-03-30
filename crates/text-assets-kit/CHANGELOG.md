# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Added shared `LazyValue` / `LazyInitError` primitives plus `bootstrap_text_resources_then_load(...)`, so domain runtime adapters can reuse the same lazy-init and bootstrap+rollback orchestration without reimplementing it per crate.

### Fixed
- `lock_bootstrap_transaction(...)` now fails closed when inspecting or canonicalizing the bootstrap root prefix fails, instead of silently deriving an unstable lock key from partial path information.
- `text-assets-kit`：Unix 下的 bootstrap advisory lock 目录不再固定落在全局 `/tmp/.text-assets-kit-bootstrap-locks`；现在优先使用用户级 runtime 目录（`$XDG_RUNTIME_DIR` 或 `/run/user/<uid>`），缺失时退回 `/tmp/.text-assets-kit-bootstrap-locks/uid-<uid>` 的每用户命名空间，避免跨用户共享同一全局锁目录。
- Kept the unix socket entry regression test under a short non-symlink temp root so hook and CI runs do not fail on host `sun_path` limits before the real validation path executes.

### Changed
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Renamed the old mixed `runtime-assets-kit` boundary to `text-assets-kit` and narrowed it to generic text-resource path validation, secure filesystem access, data-root helpers, and bootstrap/rollback primitives.
- Kept the shared text-manifest bootstrap path public so downstream domain adapters can reuse it without reaching into private modules.
- Clarified that bootstrap/rollback only serializes same-root attempts and performs best-effort cleanup for the current attempt; it does not promise crash-safe or power-loss-recovery transactions.
- Demoted the root `LazyValue` / `LazyInitError` exports to deprecated compatibility re-exports and documented the underlying lazy module as a blocking shim instead of an async runtime-facing foundation API.
