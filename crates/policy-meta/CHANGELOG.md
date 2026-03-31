# Changelog

## [Unreleased]

### Fixed

- Make `export-artifacts --check` fail closed for stale files under `schema/`, `bindings/`, and `profiles/`, and let regeneration prune stale artifacts back to the canonical checked-in set.
- Align the checked-in JSON Schema dialect with the actual `schemars` 2019-09 generator output instead of advertising 2020-12.
- Replace artifact export/check `Box<dyn Error>` returns with structured `ArtifactError` variants so callers can distinguish drift, stale artifacts, I/O failures, and JSON parse failures.
