# i18n-kit

源码入口：[`crates/i18n-kit/src/lib.rs`](../../crates/i18n-kit/src/lib.rs)

## 领域

`i18n-kit` 负责国际化目录、locale 选择和结构化文本渲染。

核心问题是：给定 locale、catalog key 和参数，如何找到正确模板并渲染出最终用户文本。

## 边界

负责：

- locale 标识与选择
- catalog key 查找
- 模板插值
- 默认 locale 与 fallback
- `structured_text_kit::StructuredText` 到最终文本的渲染

不负责：

- 资源文件落盘和 bootstrap
- prompt 管理
- ICU 级复杂模板格式
- UI 层排版

## 范围

覆盖：

- 静态 catalog
- 动态 JSON catalog
- 可热替换的全局 catalog 句柄
- CLI / argv / env locale 解析
- 结构化文本中的嵌套文本渲染

不覆盖：

- plural/select 等复杂语法
- 远程翻译服务

## 结构设计

- `src/catalog.rs`
  - `Catalog` trait，定义 locale 能力和解析契约
- `src/translation.rs`
  - `TranslationCatalog`
  - `TranslationResolution`
  - 模板插值与结构化文本渲染
- `src/locale.rs`
  - locale 规范化与标识
- `src/locale_selection.rs`
  - CLI / argv / env locale 选择
- `src/static_catalog.rs`
  - 编译期静态 JSON catalog
- `src/dynamic/`
  - 运行期 JSON catalog 加载、目录组合和约束校验
- `src/global_catalog.rs`
  - 线程安全可替换的全局 catalog 句柄

## 与其他 crate 的关系

- 依赖 `structured-text-kit` 提供的 `StructuredText` / `CatalogText` 文本原语
- 被 `runtime-assets-kit` 在 `i18n` feature 下消费

## 边界备注

有些 i18n 系统会把翻译条目称作 `message`，但这个仓库不沿用这套命名。这里需要明确：

- `i18n-kit` 依赖的不是泛化“消息系统”，而是更窄的“结构化用户文本”原语。
- `translation` / `catalog text` 在这里指的是用户可见文本条目，而不是 IM、内部通信或协议负载。
- 这样可以避免把 IM 消息、内部通信消息或协议消息混进 i18n 的领域边界。
