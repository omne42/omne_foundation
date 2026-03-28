# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced the crate-local changelog after splitting release notes by crate.
- Added crate-local direct tests for ordering/lookup, freeform vs diagnostic rendering, serde shape, and nesting-limit validation so `structured-text-kit` no longer relies on downstream coverage for its core semantics.

### Changed
- Extracted the reusable structured-text primitives from the legacy `error-kit` boundary into `structured-text-kit`.
