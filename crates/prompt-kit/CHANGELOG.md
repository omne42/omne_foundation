# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a prompt-local `lazy_state` and duplicate bootstrap orchestration.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.

### Added
- Split prompt-directory bootstrap and lazy runtime handle logic out of the old mixed runtime-assets crate so prompt-specific behavior now lives behind its own domain crate.
