# mcp-kit

源码入口：[`crates/mcp-kit/src/lib.rs`](../../crates/mcp-kit/src/lib.rs)  
详细文档：[`crates/mcp-kit/docs/README.md`](../../crates/mcp-kit/docs/README.md)

## 领域

`mcp-kit` 负责 MCP client/runner 基建。

它在 `mcp-jsonrpc` 之上解决配置加载、连接缓存、initialize 生命周期、会话管理和安全默认值。

## 边界

负责：

- `mcp.json` 配置模型与加载
- 多 server 连接管理
- MCP initialize / initialized 生命周期
- 常用 MCP 方法的便捷封装
- server -> client handler
- trust / untrusted 安全模型
- CLI `mcpctl`

不负责：

- MCP server 实现
- 上层审批、sandbox、工具策略
- 自动重连与守护化

## 范围

覆盖：

- `stdio`
- `unix`
- `streamable_http`
- `Manager`
- `Session`
- `SharedManager`
- 常用 MCP typed wrapper

不覆盖：

- 完整 MCP schema 的所有 typed 包装
- 应用级业务工作流

## 结构设计

- `src/config/`
  - 配置文件格式、模型、加载与校验
- `src/manager/`
  - 连接建立、生命周期、handler、streamable HTTP 校验
- `src/session.rs`
  - 单个已初始化 MCP 会话
- `src/shared_manager.rs`
  - 面向共享调用方的串行包装
- `src/mcp.rs`
  - 常见 MCP method typed wrapper
- `src/security.rs`
  - `TrustMode` 和 untrusted 策略
- `src/bin/mcpctl.rs`
  - CLI 入口

## 与其他 crate 的关系

- 依赖 `mcp-jsonrpc`
- 与 `notify-kit`、`secret-kit` 不直接耦合
- 详细专题文档放在 crate 自己的 `docs/`
