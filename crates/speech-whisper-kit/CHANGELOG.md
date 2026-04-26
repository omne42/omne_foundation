# Changelog

## Unreleased

- Add reusable `whisper-rs` local transcription adapter boundary.
- Add PCM WAV validation and mono `f32` sample conversion helpers for Whisper runtimes.
- Expose `metal` and `cuda` features that forward to `whisper-rs`, so product crates do not need a direct `whisper-rs` dependency just to select GPU backends.
