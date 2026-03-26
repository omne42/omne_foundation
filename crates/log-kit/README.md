# log-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`log-kit` 负责结构化日志记录原语。

它解决的问题不是“日志后端怎么配置”，也不是“用户文本如何建模”，而是“一个日志事件在进入 `tracing` 之前，如何被建模为稳定 code、级别、可选结构化文本和机器字段”。

## 边界

负责：

- 定义稳定日志 code
- 定义日志级别
- 定义结构化日志记录与字段值
- 把日志记录桥接到 `tracing`

不负责：

- `tracing-subscriber` / sink / formatter 配置
- 错误领域语义
- 跨语言协议 DTO
- locale 渲染

## 范围

覆盖：

- Rust 进程内日志记录建模
- `StructuredText` 作为可选日志文本载体
- 受限机器字段集合
- 发射到 `tracing`

不覆盖：

- 任意 JSON value 树
- span 管理与订阅器安装
- 日志文件轮转或后端传输

## 结构设计

- `src/lib.rs`
  - 对外导出日志领域核心类型
- `src/code.rs`
  - `LogCode` 与校验
- `src/field.rs`
  - 字段值与字段名校验
- `src/record.rs`
  - `LogRecord`、`LogLevel` 与 `tracing` 发射桥接

## 与其他 crate 的关系

- 单向依赖 [`structured-text-kit`](../structured-text-kit/README.md)
- 单向建立在 `tracing` 生态之上
- 适合作为 `notify-kit` 这类需要稳定日志 code 的领域 crate 的上层日志模型
