# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Renamed the old mixed `runtime-assets-kit` boundary to `text-assets-kit` and narrowed it to generic text-resource path validation, secure filesystem access, data-root helpers, and bootstrap/rollback primitives.
- Kept the shared text-manifest bootstrap path public so downstream domain adapters can reuse it without reaching into private modules.
