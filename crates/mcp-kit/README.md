# mcp-kit

源码入口：[`src/lib.rs`](./src/lib.rs)  
详细文档：[`docs/README.md`](./docs/README.md)

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
  - MCP 配置模型、外部格式适配、领域校验，并复用 `config-kit::SchemaConfigLoader` 承接通用文件加载边界
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
- 依赖 [`config-kit`](../config-kit/README.md) 承接通用配置文件加载、路径边界、有界读取与 typed file-layer load
- 对外暴露的 `ServerNameError` 已接入 `error-kit` 的稳定错误语义
- 与 `notify-kit`、`secret-kit` 不直接耦合
- 详细专题文档放在 crate 自己的 `docs/`

独立的 MCP client/runner 基建目录（Rust workspace）。

包含：

- `mcp-jsonrpc`：JSON-RPC（stdio / unix / streamable http）client
- `mcp-kit`：`mcp.json` 解析 + MCP 连接/生命周期管理（stdio / unix / streamable http）
- `mcpctl`：基于配置的 MCP CLI（类似 “mcpctl”）

## 文档

- 文档入口：`docs/README.md`
- GitBook 目录：`docs/SUMMARY.md`
- 推荐阅读顺序：`docs/quickstart.md` → `docs/config.md` → `docs/library.md` → `docs/security.md`
- 本地预览（可选）：`cargo install mdbook --locked && mdbook serve docs --open`
- 给 LLM 用的单文件文档：`llms.txt` / `docs/llms.txt`（生成脚本：`./scripts/gen-llms-txt.sh`）

## 快速开始

```bash
# 在 mcp-kit/ 下
cargo run -p mcp-kit --features cli --bin mcpctl -- --help
```

## 配置（v1 最小 schema）

默认发现顺序（相对 `--root`，默认当前目录）：

1. `./.mcp.json`
2. `./mcp.json`

```json
{
  "version": 1,
  "servers": {
    "remote": {
      "transport": "streamable_http",
      "url": "https://example.com/mcp"
    }
  }
}
```

可选字段：

- `client.protocol_version` / `client.capabilities`：覆盖 MCP initialize 里的 client 配置。
- `client.roots`：启用 roots 能力，并自动响应 server→client 的 `roots/list`。
- `servers.<name>.stdout_log`：将 server stdout 旋转落盘（见 `mcp_jsonrpc::StdoutLog`），支持 `max_bytes_per_part` 与 `max_parts`（0 表示不做保留上限）。
- `servers.<name>.inherit_env`：仅 `transport=stdio` 生效；是否继承当前进程环境变量（默认 `false`）。当为 `false` 时会清空子进程 env（仅保留少量基础变量并再注入 `servers.<name>.env`），用于降低宿主 secrets 泄露风险。
- `transport=unix`：连接已有 unix socket MCP server（见 `servers.<name>.unix_path`）。
- `transport=streamable_http`：连接远程 MCP server（见 `servers.<name>.url` 或 `servers.<name>.sse_url + servers.<name>.http_url`），可选 `servers.<name>.bearer_token_env_var` / `servers.<name>.http_headers` / `servers.<name>.env_http_headers`。
- 安全默认（`TrustMode::Untrusted`）：仅允许连接 `https` 且非 localhost/私网的 `streamable_http`（含 DNS 解析校验，默认 fail-closed）；并拒绝发送 `Authorization`/`Cookie` 等敏感 header、拒绝读取 env secrets；需要显式信任（`--trust --yes-trust` / `TrustMode::Trusted`）才放开。

## 作为库使用

```rust
use std::time::Duration;

use mcp_kit::{mcp, Config, Manager, UntrustedStreamableHttpPolicy};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = std::env::current_dir()?;
    let config = Config::load(&root, None).await?;
    // 默认 TrustMode::Untrusted：
    // - 允许连接远程 `transport=streamable_http`（仅 https 且非 localhost/私网；不允许认证 header / env secrets）
    // - 拒绝本地 `transport=stdio|unix`（避免不可信仓库导致本地执行/本地 socket 滥用）
    // 如确需启用本地 transport 或 env secrets，显式开启：`.with_trust_mode(TrustMode::Trusted)`
    // 如需在不完全信任的前提下，收紧/放开远程出站规则，可配置 policy：
    // `.with_untrusted_streamable_http_policy(UntrustedStreamableHttpPolicy { allowed_hosts: vec!["example.com".into()], ..Default::default() })`
    //
    // `Config::load` 已隐式校验 `config.client()`；手动构造 config 时可用 `Manager::try_from_config`。
    let mut mcp = Manager::from_config(&config, "my-app", "0.1.0", Duration::from_secs(30))
        .with_untrusted_streamable_http_policy(UntrustedStreamableHttpPolicy {
            allowed_hosts: vec!["example.com".to_string()],
            ..Default::default()
        });

    let tools = mcp
        .request_typed::<mcp::ListToolsRequest>(&config, "remote", None, &root)
        .await?;

    if let Some(init) = mcp.initialize_result("remote") {
        eprintln!("server initialize: {}", serde_json::to_string_pretty(init)?);
    }

    println!("{}", serde_json::to_string_pretty(&tools)?);
    Ok(())
}
```

`Manager` 内置了 MCP 常用请求的便捷方法（`ping`、`resources/read`、`prompts/get`、`logging/setLevel` 等）；也可用 `request`/`request_typed` 发送任意自定义方法。

如需把单个 server 的会话交给其他库持有，可用 `Manager::get_or_connect_session` / `Manager::connect_*_session` 取出 `Session`，再调用 `Session::{list_tools, call_tool, read_resource}` 等。

`mcp_kit::mcp` 模块提供了一组**常用方法的轻量 typed wrapper**（参考 `docs/examples.md` 的用法示例），不覆盖完整 MCP schema；缺的部分可继续用 `serde_json::Value` 或自行实现 `McpRequest`/`McpNotification`。

## 常用命令

```bash
mcpctl list-servers
# 远程 streamable_http server（https + 非 localhost/私网 + 无认证 header/env secrets）可直接使用
mcpctl list-tools <server>

# 本地 stdio/unix server 或需要读取 env secrets 的远程 server，需要显式信任
mcpctl --trust --yes-trust list-tools <server>
mcpctl --trust --yes-trust call <server> <tool> --arguments-json '{"k":"v"}'
mcpctl --trust --yes-trust request <server> <method> --params-json '{"k":"v"}'

# 不完全信任时，也可显式放开部分出站策略（仅影响 streamable_http）
mcpctl --allow-host example.com list-tools <server>
mcpctl --allow-private-ip --allow-http list-tools <server>
```

> 提示：默认不安装到 PATH，可用 `cargo run -p mcp-kit --features cli --bin mcpctl -- ...`。

## 开发

- 在 `crates/mcp-kit/` 下启用 hooks：`bash ./scripts/setup-githooks.sh`
- Workspace gates（对齐 CI）：`cd ../.. && scripts/check-workspace.sh ci`
- 仅验证本 crate 的文档与 LLM 资产：`cd ../.. && scripts/check-workspace.sh asset-checks mcp-kit`
