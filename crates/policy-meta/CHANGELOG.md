# Changelog

## [Unreleased]

### Fixed

- Make `export-artifacts --check` fail closed for stale files under `schema/`, `bindings/`, and `profiles/`, and let regeneration prune stale artifacts back to the canonical checked-in set.
- Align the checked-in JSON Schema dialect with the actual `schemars` 2019-09 generator output instead of advertising 2020-12.
- Replace artifact export/check `Box<dyn Error>` returns with structured `ArtifactError` variants so callers can distinguish drift, stale artifacts, I/O failures, and JSON parse failures.
- Replace the `export-artifacts` public command path and the `export-schemas` / `export-types` CLI entrypoints' `Box<dyn Error>` exits with typed command/CLI error variants, so argument mistakes and artifact failures stop collapsing into erased boundaries.
- `export-artifacts` CLI now exposes typed parse and execution errors instead of flattening everything into `Box<dyn Error>`; callers can distinguish missing flag values, unknown arguments, and underlying `ArtifactError` failures.
