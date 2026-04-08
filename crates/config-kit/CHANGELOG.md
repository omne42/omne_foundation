# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Changed
- `config-kit` 通过 workspace 继承的 `omne-fs-primitives` 依赖现在补上显式 `version` 约束；manifest 导出时不再把这条跨仓 runtime 边降成无版本约束的隐式依赖。

### Changed
- `config-kit` no longer vendors `omne-fs-primitives` inside the `omne_foundation` workspace; it now depends on the canonical runtime-owned crate from `omne-runtime`, so filesystem primitives only have one cross-repo owner.
- `config-kit` 现在明确标记为 `publish = false`。在它依赖的 workspace/runtime foundation crate 形成独立 crates.io 发布链之前，本 crate 只承诺 Git / monorepo 复用边界，不再让 manifest 隐含“当前可直接单独发布”的错误信号。
- `config-kit` 在 rooted candidate root 打开失败时，不再依赖底层英文 `io::Error` 文案判断 symlink 根路径；现在会直接检查 root/已存在祖先路径的 symlink 身份并稳定映射到 `Error::SymlinkPath`。
- `config-kit::canonicalize_path_in_root(...)` 现在会先把相对 `path` 绑定到显式传入的 `root`，不再偷偷回退到进程当前工作目录；rooted canonicalize 的边界与函数名重新一致。

### Added
- `config-kit` crate: shared config foundation for bounded file loading, format detection, rooted path checks, `${ENV_VAR}` interpolation, and layered object merge.
- `SchemaConfigLoader`, `SchemaFileLayerOptions`, `LoadedSchemaConfig<T>`, and `LoadedSchemaLayer` for typed business-schema loading on top of the shared config boundary.
- `ConfigFormatSet`, `ConfigDocument::parse_as(...)`, `load_typed_config_file(...)`, and `try_load_typed_config_file(...)` for strict allowed-format business schema parsing on top of loaded config documents.

### Changed
- `canonicalize_path_in_root(...)` is no longer part of the public `config-kit` API surface; rooted path checks stay internal to the crate's fail-closed loaders instead of exposing a check-then-use helper.
- Rooted candidate config discovery is now capability-style fail-closed: candidate paths must stay relative to the declared root, may not use absolute paths or `..`, and may not cross intermediate directory symlinks; callers that need an explicit external file must use an explicit file layer.
- `SchemaConfigLoader` now reports missing required explicit and candidate file layers through the same `required config layer ... not found` error family instead of mixing `Io` and `InvalidOptions`.
- `try_load_config_document(...)` and rooted candidate discovery now check file existence before rejecting extensionless paths, so missing `try_*` targets and missing fallback candidates correctly return `None` instead of surfacing premature `UnsupportedFormat` errors, while existing extensionless files still require an explicit or default format.
