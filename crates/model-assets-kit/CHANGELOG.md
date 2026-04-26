# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced `model-assets-kit` as the shared model manifest/source/capability/install-status boundary.
- Added `candleWhisper` as a future local runtime backend marker alongside `whisperRs` and compatibility sidecar execution.
- Added structured model checksums and the full official `whisper.cpp` GGML catalog helpers, including q5/q8 quantized models, `small.en-tdrz`, and `ggml-*.bin` model-id inference.
