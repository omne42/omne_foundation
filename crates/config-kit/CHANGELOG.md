# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

- `config-kit`：对 `omne-fs-primitives` 的内部依赖现在显式声明版本与 path，允许 package/publish 边界按 crate 自己的清单解析，而不再只能锁死在当前 workspace。

### Added
- `config-kit` crate: shared config foundation for bounded file loading, format detection, rooted path checks, `${ENV_VAR}` interpolation, and layered object merge.
- `SchemaConfigLoader`, `SchemaFileLayerOptions`, `LoadedSchemaConfig<T>`, and `LoadedSchemaLayer` for typed business-schema loading on top of the shared config boundary.
- `ConfigFormatSet`, `ConfigDocument::parse_as(...)`, `load_typed_config_file(...)`, and `try_load_typed_config_file(...)` for strict allowed-format business schema parsing on top of loaded config documents.

### Changed
- Rooted candidate config discovery is now capability-style fail-closed: candidate paths must stay relative to the declared root, may not use absolute paths or `..`, and may not cross intermediate directory symlinks; callers that need an explicit external file must use an explicit file layer.
- `SchemaConfigLoader` now reports missing required explicit and candidate file layers through the same `required config layer ... not found` error family instead of mixing `Io` and `InvalidOptions`.
- `try_load_config_document(...)` and rooted candidate discovery now check file existence before rejecting extensionless paths, so missing `try_*` targets and missing fallback candidates correctly return `None` instead of surfacing premature `UnsupportedFormat` errors, while existing extensionless files still require an explicit or default format.
