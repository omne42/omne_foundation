# Changelog

## [Unreleased]

- `policy-meta`: `export-artifacts -- --check` now fails when `schema/` or `bindings/` contains stale generated entries, and export rewrites those directories as exact generated sets instead of leaving old files behind.
