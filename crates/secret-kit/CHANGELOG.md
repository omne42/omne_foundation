# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Stabilize Linux process-group cleanup tests by detaching background-command stdio, tracking PID identity to avoid `/proc` reuse false negatives, and relaxing cleanup polling windows so slower CI runners do not spuriously fail `secret-kit` quality gates while preserving the same cleanup assertions.
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Kept `secret-kit` focused on secret-specific semantics while moving shared process-tree primitives out to the systems layer and preserving structured error texts.
- Retry Unix `ETXTBSY` (`Text file busy`) command spawns briefly so freshly materialized builtin CLI shims do not introduce flaky secret resolution failures.
- Move the Unix `ETXTBSY` spawn-retry backoff onto Tokio time so async secret resolution no longer blocks executor workers while preserving the same retry contract.
- Mark deterministic local file/input failures as `DoNotRetry` while keeping transient I/O and CLI timeout/spawn failures retryable so upstream callers stop misclassifying secret resolution incidents.
