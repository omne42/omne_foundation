# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Introduced the crate-local changelog after splitting release notes by crate.
- Added the protocol-facing structured-text data model alongside the extracted structured-text core crate.

### Changed
- Forward-compatible `CatalogArgValueRef` mapping now emits an explicit `unsupported` DTO value and returns a diagnostic reconstruction error instead of panicking on unknown runtime variants.
