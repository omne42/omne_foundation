# error-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`error-kit` 负责结构化错误原语。

它解决的问题不是“文本如何表达”，也不是“错误如何通过线协议传输”，而是“一个错误对象自身要携带哪些稳定语义”。

这里的核心模型是：

- `ErrorCode`：稳定机器语义错误码
- `ErrorCategory`：错误类别
- `ErrorRetryAdvice`：是否建议重试
- `ErrorRecord`：把上面这些元信息与 `StructuredText`、可选诊断文本、可选 source 组合起来

## 边界

负责：

- 定义稳定错误码与其校验规则
- 定义错误类别和重试建议
- 定义结构化错误对象
- 承载用户可见文本与诊断文本
- 承载本地运行时 source error 链接

不负责：

- 结构化文本原语本身
- locale 渲染
- JSON / TypeScript / schema 协议输出
- 日志记录模型

## 范围

覆盖：

- Rust 进程内错误语义建模
- `StructuredText` 作为错误文本载体
- source error 链接
- 适合被各领域 crate 包装或组合的通用错误记录

不覆盖：

- 错误对象的跨语言线协议
- 任意动态 map / JSON 风格 detail bag
- 日志事件、span、trace 等观测语义

## 结构设计

- `src/lib.rs`
  - 对外导出错误领域核心类型
- `src/code.rs`
  - `ErrorCode` 与错误码校验
- `src/record.rs`
  - `ErrorRecord`、类别、重试建议、Display / source 语义

## 与其他 crate 的关系

- 单向依赖 [`structured-text-kit`](../structured-text-kit/README.md)
- 被 [`error-protocol`](../error-protocol/README.md) 用作协议源模型
- 已被 `secret-kit` 用作稳定错误语义基座
- 适合作为其他领域 crate 的公共错误基座
