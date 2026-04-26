# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced `audio-input-kit` as the shared audio input device/config/session event boundary.
- Added CPAL native input device listing and mono `f32` microphone stream adapter.
- Added stable audio input error kind `code()` and `retryable()` helpers plus JSON compatibility coverage for input config DTOs.

### Changed
- CPAL native stream startup now honors `AudioInputConfig.format` instead of silently using the device default format.
- CPAL stream callback errors are retained on `CpalInputStream` and exposed via `drain_errors()`.
- CPAL stream callback error retention is now bounded and keeps the latest errors, avoiding unbounded memory growth when callers do not drain immediately.
