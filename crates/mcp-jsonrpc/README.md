# mcp-jsonrpc

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`mcp-jsonrpc` 负责 JSON-RPC 2.0 client 传输层。

它是 MCP 友好的 transport 基础库，但本身不承担 MCP 语义管理。

## 边界

负责：

- stdio transport
- unix domain socket transport
- streamable HTTP transport
- request / response / notification IO 编排
- server -> client requests 与 notifications 的接收缓冲
- 消息大小和队列边界

不负责：

- `mcp.json` 配置
- MCP initialize
- tool/resource/prompt typed API
- JSON-RPC server

## 范围

覆盖：

- 有界队列
- 单消息大小限制
- stdout 日志旋转落盘
- streamable HTTP 的 SSE + POST 桥接
- transport 级公开错误的稳定语义映射

不覆盖：

- 自动重连
- 丰富 typed schema
- 更高层会话管理

## 结构设计

- `src/lib.rs`
  - client 主体、消息收发循环和组合根
- `src/transport.rs`
  - transport 选项、limits、stdout log 配置
- `src/error.rs`
  - JSON-RPC transport 公开错误边界与稳定分类
- `src/stdout_log.rs`
  - stdout 分段日志
- `src/streamable_http.rs`
  - streamable HTTP transport 实现

## 与其他 crate 的关系

- 被 `mcp-kit` 消费，作为底层 transport 层
- 依赖 [`error-kit`](../error-kit/README.md) 提供公开错误的稳定错误码、类别和重试语义
- 自身不感知 `mcp-kit` 的配置模型和安全策略
