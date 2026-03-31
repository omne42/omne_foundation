# Changelog

## [Unreleased]

- `omne-process-primitives`：移除 `publish = false`，把 crate 从 monorepo-only 状态提升到可单独打包的 foundation 原语边界。

- stop formatting full host recipe `stdout`/`stderr` into `HostRecipeError::Display`; surface only exit status and captured byte counts while preserving raw `Output` for callers
- stop draining oversized stdout/stderr streams after the capture limit is reached, while still allowing outputs that end exactly on the capture limit
