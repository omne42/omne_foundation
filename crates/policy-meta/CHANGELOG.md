# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Fixed

- Keep the `thiserror`-backed artifact error boundary declared in `Cargo.toml`, so `ArtifactGenerationError` and `ArtifactError` continue compiling from the library side.
- Re-export artifact generation and drift-check helpers from the crate root, so `export-artifacts`, `export-schemas`, and `export-types` can rely on `policy_meta::{...}` as the public entrypoint.
- Keep schema, TypeScript bindings, and baseline profiles under the same artifact export/check surface, so `--check` covers all checked-in policy-meta assets.
