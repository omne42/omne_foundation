# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced `model-assets-kit` as the shared model manifest/source/capability/install-status boundary.
- Added `candleWhisper` as a future local runtime backend marker alongside `whisperRs` and compatibility sidecar execution.
- Added structured model checksums and the full official `whisper.cpp` GGML catalog helpers, including q5/q8 quantized models, `small.en-tdrz`, and `ggml-*.bin` model-id inference.

### Changed
- Documented `ModelStore` as a narrow install orchestration boundary backed by injected runtime primitives.
- Tightened legacy local model discovery to regular non-symlink `ggml-*.bin` files instead of accepting any file in a model directory.
- Added JSON compatibility coverage for model manifest DTOs.
- `ModelStore` now rejects symlinked `model.json` files and metadata paths that point outside the model directory, keeping local model discovery inside the store boundary.
- `ModelStore` now rejects existing non-regular model destination paths before reuse or checksum verification, so install cannot accept a symlinked model file as already installed.
- `ModelStore` now treats `size_bytes` as an installed-file verification contract, preventing wrong-size files from being reused or imported.
- `ModelSource::Https` download candidates now require credential-free `https://` URLs before reaching the injected downloader.
- Hugging Face source URLs are now built from validated path segments instead of raw string interpolation, preventing malformed manifest fields from injecting query or reserved path semantics.
