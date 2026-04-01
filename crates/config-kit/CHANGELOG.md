# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `config-kit` no longer vendors `omne-fs-primitives` inside the `omne_foundation` workspace; it now depends on the canonical runtime-owned crate from `omne-runtime`, so filesystem primitives only have one cross-repo owner.
- `config-kit` 现在明确标记为 `publish = false`。在它依赖的 workspace/runtime foundation crate 形成独立 crates.io 发布链之前，本 crate 只承诺 Git / monorepo 复用边界，不再让 manifest 隐含“当前可直接单独发布”的错误信号。

### Added
- `config-kit` crate: shared config foundation for bounded file loading, format detection, rooted path checks, `${ENV_VAR}` interpolation, and layered object merge.
- `SchemaConfigLoader`, `SchemaFileLayerOptions`, `LoadedSchemaConfig<T>`, and `LoadedSchemaLayer` for typed business-schema loading on top of the shared config boundary.
- `ConfigFormatSet`, `ConfigDocument::parse_as(...)`, `load_typed_config_file(...)`, and `try_load_typed_config_file(...)` for strict allowed-format business schema parsing on top of loaded config documents.

### Changed
- Rooted candidate config discovery is now capability-style fail-closed: candidate paths must stay relative to the declared root, may not use absolute paths or `..`, and may not cross intermediate directory symlinks; callers that need an explicit external file must use an explicit file layer.
- `SchemaConfigLoader` now reports missing required explicit and candidate file layers through the same `required config layer ... not found` error family instead of mixing `Io` and `InvalidOptions`.
- `try_load_config_document(...)` and rooted candidate discovery now check file existence before rejecting extensionless paths, so missing `try_*` targets and missing fallback candidates correctly return `None` instead of surfacing premature `UnsupportedFormat` errors, while existing extensionless files still require an explicit or default format.
