# Changelog

## Unreleased

- Add reusable `whisper-rs` local transcription adapter boundary.
- Add PCM WAV validation and mono `f32` sample conversion helpers for Whisper runtimes.
- Expose `metal` and `cuda` features that forward to `whisper-rs`, so product crates do not need a direct `whisper-rs` dependency just to select GPU backends.
- Model and WAV input paths are now opened through no-follow runtime filesystem primitives and reject symlinked or non-regular files before local transcription work.
- Add JSON compatibility coverage for runtime DTOs and stable `WhisperRuntimeError::code()` / `retryable()` mappings.
