# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- Established crate-local changelog ownership now that `omne_foundation` tracks release notes per crate instead of at the repository root.
- Hardened runtime asset loading and bootstrap behavior so missing roots, rollback scope, and shared byte limits are handled more explicitly.
