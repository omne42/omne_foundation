# mcpctl

`mcpctl` 是一个基于 `mcp.json` 的 MCP client/runner（config-driven; stdio/unix/streamable_http）。

它的定位是：

- 快速验证配置是否正确（`list-servers`）
- 探测 server 暴露的能力（tools/resources/prompts）
- 发送 raw request/notification 进行调试

## 运行方式

当前仓库内（推荐）：

```bash
cd crates/mcp-kit
cargo run -p mcp-kit --features cli --bin mcpctl -- --help
```

> 注意：`mcpctl` 通过 feature `cli` 启用，避免 library 依赖方被迫引入 `clap`。

## 全局参数（flags）

- `--root <path>`：workspace root；用于相对路径解析，并作为 stdio server 的工作目录
- `--config <path>`：覆盖配置文件路径（绝对或相对 `--root`；默认要求位于 `--root` 内）
  - 一旦定位到具体 config 文件，文件内相对 `unix_path` / `stdout_log.path` 会按该 config 文件所在目录解析；默认发现到的是 `--root` 下的 `mcp.json`，所以常见场景里两者等价
- `--json`：输出紧凑 JSON（默认 pretty JSON）
- `--timeout-ms <ms>`：per-request 超时（默认 30000）

安全相关：

- `--trust`：完全信任 `mcp.json`（允许 stdio/unix、允许读取 env secrets、允许发送认证 header；需要配合 `--yes-trust`）
- `--yes-trust`：确认你理解 `--trust` 的风险（没有它会拒绝运行）
- `--allow-config-outside-root`：允许 `--config` 指向 `--root` 之外（默认拒绝；仅建议在可信路径下使用）
- `--allow-stdout-log-outside-root`：允许 `stdout_log.path` 写到 `--root` 之外（默认拒绝；仅建议在可信配置下使用）
- `--show-argv`：`list-servers` 时输出 stdio `argv` 明文（默认不输出；避免把 token/key 打进终端/CI）
- `--allow-http`：Untrusted 下允许连接 `http://`（默认只允许 https）
- `--allow-localhost`：Untrusted 下只允许连接 `localhost` / `localhost.localdomain` / `*.localhost`
- `--allow-private-ip`：Untrusted 下允许连接非公网 IP 字面量
- `--no-dns-check`：显式关闭默认启用的 DNS 校验（更不安全）
- `--dns-timeout-ms <ms>`：DNS lookup 超时（仅在 DNS 校验开启时生效；默认 2000）
- `--dns-fail-open`：DNS lookup 失败/超时时不拦截（fail-open；仅在 DNS 校验开启时生效；默认 fail-closed）
- `--allow-host <host>`：Untrusted 下设置 host allowlist（可重复；默认 DNS 校验已开启）

> `--allow-*` / `--no-dns-check` 只影响 `transport=streamable_http`，不会放开 stdio/unix（它们需要 `--trust --yes-trust`）。
>
> 注意：`--allow-host` allowlist **不会**覆盖 `localhost/localdomain/单标签 host` 的拦截；`--allow-localhost` 只会放开 `localhost` / `localhost.localdomain` / `*.localhost`。如果你确实要连接 `*.local`、`*.localdomain` 或单标签 host，只能显式使用 `--trust --yes-trust`。

## 子命令（subcommands）

### list-servers

列出解析后的配置（包含 `client` 与 servers 的关键字段），用于确认最终生效值：

```bash
cargo run -p mcp-kit --features cli --bin mcpctl -- list-servers
```

说明：

- 为了避免意外打印 secrets，`list-servers` 默认不输出 stdio `argv` 明文；如需查看，显式加 `--show-argv`。
- 同样地，`list-servers` 对 `env/http_headers/secret_http_headers` 只输出 key 列表（`env_keys/http_header_keys/secret_http_header_keys`），不输出具体值；bearer auth 只暴露 `has_bearer_token_secret` 布尔位。

### list-tools / list-resources / list-prompts

```bash
cargo run -p mcp-kit --features cli --bin mcpctl -- list-tools remote
cargo run -p mcp-kit --features cli --bin mcpctl -- list-resources remote
cargo run -p mcp-kit --features cli --bin mcpctl -- list-prompts remote
```

### call

```bash
cargo run -p mcp-kit --features cli --bin mcpctl -- call remote my.tool --arguments-json '{"k":"v"}'
```

### request（raw JSON-RPC request）

```bash
cargo run -p mcp-kit --features cli --bin mcpctl -- request remote tools/list
cargo run -p mcp-kit --features cli --bin mcpctl -- request remote resources/read --params-json '{"uri":"file:///path/to/file"}'
```

### notify（raw JSON-RPC notification）

```bash
cargo run -p mcp-kit --features cli --bin mcpctl -- notify remote notifications/initialized
```

## 常见用法组合

- 远程 server（https + 非 localhost/私网）：默认可用
- 本地 stdio/unix 或需要读取 env secrets：加 `--trust --yes-trust`
- 不完全信任但需要放开部分出站：使用 `--allow-host/--allow-http/...`

安全细节见 [`安全模型`](security.md)。
