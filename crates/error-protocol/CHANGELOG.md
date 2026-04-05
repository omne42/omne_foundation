# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced `error-protocol` as the schema and wire-format layer for `error-kit`.

### Changed
- Forward-compatible protocol mapping for `ErrorCategory` / `ErrorRetryAdvice` now degrades unknown runtime variants to explicit `unknown` DTO values instead of panicking.
- Deserializing `ErrorCategoryData::Unknown` / `ErrorRetryAdviceData::Unknown` into `error-kit::ErrorRecord` now fails structurally instead of silently rewriting them to `Internal` / `DoNotRetry`.
