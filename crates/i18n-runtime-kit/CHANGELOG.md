# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Reused `text-assets-kit` shared lazy-init and bootstrap+rollback primitives instead of maintaining a second copy inside `i18n-runtime-kit`.
- Clarified that the shared bootstrap/rollback primitives used here provide best-effort cleanup for the current attempt, not crash-safe transactions.
- Clarified `GlobalCatalog` as the runtime-facing canonical handle and downgraded the root `LazyCatalog` export to a deprecated blocking compatibility path.

### Fixed
- Kept the unix socket entry regression test under a short non-symlink temp root so pre-commit and CI still exercise directory validation instead of failing on host socket path-length limits.

### Added
- Split runtime i18n asset loading, bootstrap, and lazy/global catalog handles out of the old mixed runtime-assets crate so the i18n domain now owns its own runtime adapter boundary.
- Added runtime-owned CLI / argv locale parsing plus `CliLocaleError`, so command-line locale input no longer leaks back into `i18n-kit`.
