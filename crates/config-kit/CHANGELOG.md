# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- `config-kit` crate: shared config foundation for bounded file loading, format detection, rooted path checks, `${ENV_VAR}` interpolation, and layered object merge.
- `SchemaConfigLoader`, `SchemaFileLayerOptions`, `LoadedSchemaConfig<T>`, and `LoadedSchemaLayer` for typed business-schema loading on top of the shared config boundary.
- `ConfigFormatSet`, `ConfigDocument::parse_as(...)`, `load_typed_config_file(...)`, and `try_load_typed_config_file(...)` for strict allowed-format business schema parsing on top of loaded config documents.

### Changed
- Rooted candidate config discovery is now capability-style fail-closed: candidate paths must stay relative to the declared root, may not use absolute paths or `..`, and may not cross intermediate directory symlinks; callers that need an explicit external file must use an explicit file layer.
- `SchemaConfigLoader` now reports missing required explicit and candidate file layers through the same `required config layer ... not found` error family instead of mixing `Io` and `InvalidOptions`.
