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
- streamable HTTP 的显式代理策略边界

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
  - crate 入口
  - 对外 API re-export
- `src/client.rs`
  - client 主体
  - request / response 编排
  - pending request / close state
- `src/reader.rs`
  - reader loop
  - 入站 JSON-RPC 分发
  - 行读取与 batch 处理辅助
- `src/options.rs`
  - transport 选项
  - limits
- `src/error.rs`
  - 错误类型与 `error-kit` 映射
- `src/runtime.rs`
  - Tokio time driver 前置检查
- `src/detached.rs`
  - 无 runtime 场景的后台派发与 fail-closed close 辅助
- `src/stdout_log.rs`
  - stdout 分段日志
- `src/streamable_http.rs`
  - streamable HTTP transport 实现

## 与其他 crate 的关系

- 被 `mcp-kit` 消费，作为底层 transport 层
- 依赖 [`error-kit`](../error-kit/README.md) 提供公开错误的稳定错误码、类别和重试语义
- 自身不感知 `mcp-kit` 的配置模型和安全策略

## streamable HTTP 网络策略

`StreamableHttpOptions` 当前对代理和 redirects 采用显式、安全优先的配置面：

- `follow_redirects` 默认 `false`
- `proxy_mode` 默认 [`IgnoreSystem`](./src/options.rs)，不会自动读取 `HTTP_PROXY` / `HTTPS_PROXY`
- 如果调用方确实需要走系统代理，可显式设置 `StreamableHttpProxyMode::UseSystem`

补充说明：

- 当 `enforce_public_ip = true` 时，底层 pinned public-IP 路径仍会禁用代理，以避免把实际 socket 重定向到中间代理端点
