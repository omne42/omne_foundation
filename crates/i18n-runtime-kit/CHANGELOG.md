# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- Split runtime i18n asset loading, bootstrap, and lazy/global catalog handles out of the old mixed runtime-assets crate so the i18n domain now owns its own runtime adapter boundary.
- Added runtime-owned CLI / argv locale parsing plus `CliLocaleError`, so command-line locale input no longer leaks back into `i18n-kit`.
