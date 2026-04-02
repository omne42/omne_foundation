# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced `log-kit` as a structured log-record layer built on top of `tracing`.

### Changed
- `log-kit` now emits `LogRecord` values through real tracing targets and per-field event metadata instead of collapsing custom fields into a single debug blob, so subscribers can route by target and observe flattened structured fields.
