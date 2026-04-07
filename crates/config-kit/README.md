# config-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`config-kit` 负责通用配置边界。

它解决的问题不是“某个产品的配置 schema 长什么样”，而是“配置文件如何被安全读取、识别格式、解析、层叠和解释”。

## 边界

负责：

- 配置文件格式识别（JSON / TOML / YAML）
- 有界、fail-closed、no-follow 的配置文件读取
- rooted candidate path 边界检查与越界拒绝
- rooted candidate file discovery（relative-only，拒绝绝对路径、`..` 和中间目录 symlink）
- 严格 `${ENV_VAR}` 插值
- 递归对象 merge 与变更路径报告
- 面向业务 schema 的高层 typed loader

不负责：

- 应用级配置 schema
- CLI flag 到配置字段的业务映射
- 仓库特有的默认目录约定
- 配置编辑工作流、交互式向导或审批语义

## 范围

覆盖：

- `ConfigFormat`
- `ConfigFormatSet`
- `ConfigLoadOptions`
- `ConfigDocument`
- `find_config_document(...)`
- `interpolate_env_placeholders(...)`
- `merge_config_layers(...)`
- `load_typed_config_file(...)`
- `try_load_typed_config_file(...)`
- `ConfigDocument::parse_as(...)`
- `SchemaConfigLoader`
- `SchemaFileLayerOptions`
- `LoadedSchemaConfig<T>`

不覆盖：

- 宽松别名兼容
- 自定义模板语言
- 领域特有的配置校验规则

## 结构设计

- `src/format.rs`
  - 格式识别、typed parse / value parse、render
- `src/load.rs`
  - 文件读取上限、no-follow regular-file open、路径发现与 rooted path 边界检查
- `src/typed.rs`
  - 业务 schema 的严格格式限制与 typed parse helper
- `src/env.rs`
  - 严格 `${ENV_VAR}` 插值
- `src/merge.rs`
  - 递归对象 merge、layer step 与变更路径报告
- `src/schema.rs`
  - 面向业务 schema 的高层 loader：defaults / file layers / env lookup / typed deserialize / explain
- `src/error.rs`
  - 稳定错误类型

## 高层 Schema 封装

`config-kit` 不拥有任何业务 schema，但它应该让业务 schema 的接入足够直接。

这层能力的目标是把下面这类重复模式收敛掉：

- 先塞一层 defaults
- 再在某个 root 下按顺序找 `config_local.toml` / `config.toml`
- 某些文件层允许 `${ENV_VAR}` 插值
- 最后 merge 成一个 `serde` typed struct
- 同时保留 merged JSON value 和每层 `changed_paths`，方便做 config explain

常见入口：

- `load_typed_config_file::<T>(...)`
- `ConfigDocument::parse_as::<T>(...)`
- `SchemaConfigLoader::add_serializable_layer(...)`
