# Changelog

## [Unreleased]

### Fixed

- Make `export-artifacts --check` fail closed for stale files under `schema/`, `bindings/`, and `profiles/`, and let regeneration prune stale artifacts back to the canonical checked-in set.
