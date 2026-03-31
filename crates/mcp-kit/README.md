# mcp-kit

源码入口：[`src/lib.rs`](./src/lib.rs)  
详细文档：[`docs/README.md`](./docs/README.md)

## 领域

`mcp-kit` 负责 MCP client/runner 基建。

它建立在 `mcp-jsonrpc` 之上，收口三类稳定能力：

- `mcp.json` 配置模型与加载
- 多 server 连接、initialize 生命周期与会话交接
- `TrustMode` / untrusted 默认值与 `mcpctl` CLI

## 边界

负责：

- `stdio`、`unix`、`streamable_http` 三类 transport 的 MCP client 侧接线
- `Manager`、`Session`、`shared::SharedManager` 和常用 typed wrapper
- server -> client handler 与 roots 支撑
- MCP 配置加载、安全校验和连接缓存

不负责：

- MCP server 实现
- 审批、sandbox、工具策略或业务工作流
- 后台自动重连循环、daemon 化和上层编排

补充边界：

- `Manager` / `shared::SharedManager` 会在 `request` / `notify` / `get_or_connect*` 这类配置驱动入口里按需 `connect + initialize`，并在已断开的缓存连接失效后重新建立连接。
- 这不等于后台自动重连；crate 不会自己常驻守护某个连接并在链路抖动时持续重试。

## 结构地图

- [`src/config/`](./src/config/)
  - MCP 配置模型、文件加载和领域校验
- [`src/manager/`](./src/manager/)
  - 连接建立、生命周期、handler、streamable HTTP 校验
- [`src/session.rs`](./src/session.rs)
  - 单个已初始化会话
- [`src/shared_manager.rs`](./src/shared_manager.rs)
  - 面向共享调用方的 single-flight 生命周期包装，通过 `mcp_kit::shared::SharedManager` 暴露
  - 同时提供 handler 子任务的显式 scope 继承入口
- [`src/error.rs`](./src/error.rs)
  - crate 级公开错误边界，暴露稳定的 `ErrorKind` / `Result`
- [`src/mcp.rs`](./src/mcp.rs)
  - 常用 MCP method 的轻量 typed wrapper
- [`src/security.rs`](./src/security.rs)
  - `TrustMode` 和 untrusted 策略
- [`src/bin/mcpctl.rs`](./src/bin/mcpctl.rs)
  - CLI 入口

## 文档入口

- 快速开始：[`docs/quickstart.md`](./docs/quickstart.md)
- 配置说明：[`docs/config.md`](./docs/config.md)
- 作为库使用：[`docs/library.md`](./docs/library.md)
- 安全模型：[`docs/security.md`](./docs/security.md)
- 示例与常见模式：[`docs/examples.md`](./docs/examples.md)
- 文档目录：[`docs/SUMMARY.md`](./docs/SUMMARY.md)

README 只保留 crate 级地图；配置 schema、CLI 用法、示例和更细的安全说明统一下沉到 `docs/`。

## 开发入口

- 本 crate 文档与资产检查：`cd ../.. && scripts/check-workspace.sh asset-checks mcp-kit`
- Workspace 基线：`cd ../.. && scripts/check-workspace.sh ci`
- CLI 帮助：`cargo run -p mcp-kit --features cli --bin mcpctl -- --help`
