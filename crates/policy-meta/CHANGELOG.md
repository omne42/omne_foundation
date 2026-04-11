# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Fixed

- Restore the typed artifact-generation error dependency and keep `ArtifactGenerationError` on the library side, so `policy-meta` compiles with its explicit `thiserror` boundary again.
- Re-expose artifact file generation and drift-check helpers under `policy_meta::artifacts::*` instead of the crate root, so the export binaries keep compiling without widening the stable contract surface.
- Keep the checked-in changelog as a live crate artifact so repository changelog gates still match the README and current crate layout.
