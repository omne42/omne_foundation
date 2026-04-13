# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Fixed
- `ResolveLocaleError` structured-text construction no longer panics when schema validation fails; it now degrades to freeform fallback text so locale resolution errors remain renderable.

### Changed
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Narrowed `i18n-kit` around translation/catalog responsibilities and continued the move away from the old generic structured-error boundary.
- Runtime catalog handle types (`LazyCatalog` / `GlobalCatalog` plus their initialization error wrappers) moved out to `i18n-runtime-kit`, so `i18n-kit` stays focused on pure catalog, locale, and structured-text semantics.
- Locale source path and size/count policy helpers are now exposed from `i18n-kit` so runtime adapters can reuse one source of truth.
- CLI / argv locale parsing and CLI-specific locale errors moved out to `i18n-runtime-kit`, leaving `ResolveLocaleError` as a pure catalog locale-resolution error.
