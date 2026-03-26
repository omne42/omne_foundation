# error-protocol

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`error-protocol` 是 `error-kit` 的线协议映射层。

它把 Rust 进程内的错误对象转换成适合 JSON Schema、TypeScript 和跨语言边界的数据结构。

## 边界

负责：

- `ErrorRecord` 的 DTO 表达
- 错误类别与重试建议的协议表示
- Rust 错误模型与协议 DTO 的双向转换
- 导出 TypeScript 绑定

不负责：

- 错误对象的生成策略
- 本地 source error 链接
- locale 渲染
- 更高层 RPC envelope

## 范围

覆盖：

- `ErrorCode`
- 错误类别
- 重试建议
- 用户文本与诊断文本
- 协议层输入校验与类型转换错误
- `ErrorData` 及其错误专属 DTO

不覆盖：

- source error 的跨进程传输
- 独立于错误模型之外的业务协议
- 通用 `StructuredTextData` / `CatalogArgData` 协议类型的转手导出

## 结构设计

- `src/lib.rs`
  - `ErrorData`
  - `ErrorCategoryData`
  - `ErrorRetryAdviceData`
  - `From` / `TryFrom` 转换与协议错误
- `bindings/`
  - 生成后的 TypeScript 绑定

## 与其他 crate 的关系

- 单向依赖 [`error-kit`](../error-kit/README.md)
- 单向依赖 [`structured-text-protocol`](../structured-text-protocol/README.md)
- 通用结构化文本 DTO 由 [`structured-text-protocol`](../structured-text-protocol/README.md) 直接提供，不再经由 `error-protocol` 转手
- 适合作为 Rust 与前端 / 其他语言之间的结构化错误桥接层
