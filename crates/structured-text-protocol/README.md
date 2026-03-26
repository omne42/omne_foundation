# structured-text-protocol

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`structured-text-protocol` 是 `structured-text-kit` 的线协议映射层。

它把 Rust 内部的结构化文本模型转换成适合 JSON Schema、TypeScript 和跨语言边界的数据结构。

## 边界

负责：

- `StructuredText` 的 DTO 表达
- 参数和值类型的协议表示
- Rust 模型与协议 DTO 的双向转换
- 导出 TypeScript 绑定

不负责：

- 结构化文本的生成策略
- locale 渲染
- RPC envelope 或更高层 API 协议

## 范围

覆盖：

- `Catalog` / `Freeform` 两种文本形态
- 文本、布尔、整数、嵌套文本等参数值
- 协议层输入校验与类型转换错误

不覆盖：

- `structured-text-kit` 的内部实现细节
- 独立于结构化文本模型之外的业务协议

## 结构设计

- `src/lib.rs`
  - `StructuredTextData`
  - `CatalogArgData`
  - `CatalogArgValueData`
  - `From` / `TryFrom` 转换与协议错误
- `bindings/`
  - 生成后的 TypeScript 绑定

## 与其他 crate 的关系

- 单向依赖 [`structured-text-kit`](../structured-text-kit/README.md)
- 被 [`error-protocol`](../error-protocol/README.md) 用作错误文本字段的协议桥接层
- 适合作为 Rust 与前端 / 其他语言之间的结构化文本桥接层
