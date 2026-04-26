# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced `speech-transcription-kit` as the shared speech transcription request/result boundary.
- Added shared audio asset and transcription job DTOs so products can converge on one capture/transcription history shape.
- Re-exported `audio-media-kit::AudioAssetRef` so transcription jobs share the audio asset boundary.
- Added provider/model descriptors, provider capability DTOs, and structured transcription errors.
- Added a provider registry DTO with provider/model lookup helpers and provider-level default model hints.
