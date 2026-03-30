# i18n-runtime-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`i18n-runtime-kit` 负责 runtime i18n adapter 边界。

它把目录/manifest 驱动的运行时文本资源接到 `i18n-kit` 的 catalog 语义上，并提供可长期持有的 runtime catalog 句柄供上层使用。

## 边界

负责：

- 基于目录加载动态 i18n catalog
- 基于 manifest bootstrap 并重建 i18n catalog
- dynamic catalog reload
- CLI / argv / env locale 输入解析
- `GlobalCatalog`
- 兼容层 `LazyCatalog`
- runtime 初始化错误与 locale 解析错误包装

不负责：

- locale 语义、catalog fallback 和 structured text 渲染规则本身
- 通用文本资源/data root/secure fs 基础能力
- prompt 目录管理
- UI 层排版

## 范围

覆盖：

- `bootstrap_i18n_catalog(...)`
- `load_i18n_catalog_from_directory(...)`
- `reload_i18n_catalog_from_directory(...)`
- `resolve_locale_from_cli_args(...)`
- `resolve_locale_from_argv(...)`
- `GlobalCatalog`
- 兼容层 `LazyCatalog`
- `CatalogInitError`
- `CliLocaleError`
- `CatalogLocaleError`

不覆盖：

- 静态 catalog 定义
- locale source 路径和模板语义规则的制定

## 结构设计

- `src/i18n.rs`
  - 目录型 catalog 加载、bootstrap、reload 与错误映射
- `src/lazy_catalog.rs`
  - 仅保留给迁移路径的阻塞式 lazy catalog 兼容层
- `src/global_catalog.rs`
  - 可热替换、runtime-facing 的 canonical catalog 句柄
- `src/locale_selection.rs`
  - CLI / argv locale 解析与 env fallback 选择
- `src/catalog_error.rs`
  - runtime 初始化错误、CLI locale 错误和句柄级 locale 错误包装
- 共享懒初始化并发控制与 best-effort bootstrap/rollback 流程
  - 由 [`text-assets-kit`](../text-assets-kit/README.md) 提供通用原语，`i18n-runtime-kit` 只保留 i18n 域加载与错误映射

## 与其他 crate 的关系

- 依赖 [`i18n-kit`](../i18n-kit/README.md) 提供 catalog / locale / structured text 语义
- 依赖 [`text-assets-kit`](../text-assets-kit/README.md) 提供文本资源、目录扫描、bootstrap 与 secure fs 边界
- manifest 与文本资源类型由 [`text-assets-kit`](../text-assets-kit/README.md) 直接提供，不再从 `i18n-runtime-kit` 根导出
- 刻意不把这些 runtime adapter 回塞到 `i18n-kit`，避免纯语义层重新沾上 CLI/runtime I/O
