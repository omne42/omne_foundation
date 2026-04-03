# 配置（mcp.json v1）

本章描述 `mcp_kit::Config` 支持的 `mcp.json`（v1）schema、默认发现顺序与各字段约束。

> 说明：`mcp.json v1` schema 是 fail-closed（`deny_unknown_fields`）。这对安全非常重要：拼写错误不会被静默忽略。
>
> `Config::load` 现在只接受 canonical `mcp.json v1`。旧的 `mcpServers` wrapper、直接 server map、`plugin.json` 等非 canonical 形态都会被 fail-closed 拒绝。

## 文件发现顺序

默认发现顺序（均相对 `--root`，默认当前工作目录）：

1. `./.mcp.json`
2. `./mcp.json`

CLI 可用 `--config <path>` 覆盖（绝对或相对 `--root`）。

当 `--config`（或库层 override path）最终指向了一个具体配置文件后，文件内 server 级相对路径会按“该 config 文件所在目录”解析：

- `servers.<name>.unix_path`
- `servers.<name>.stdout_log.path`

默认发现到的 `.mcp.json` / `mcp.json` 就位于 `--root` 下，所以常见场景里它和“相对 `--root`”等价；只有显式加载嵌套目录或 `--root` 外部的 config 时，这个区别才会体现出来。

> 保护性限制：为避免异常/恶意配置导致内存放大，`mcp.json`（以及 `.mcp.json`）文件大小上限为 **4MiB**；超过会 fail-closed 报错。
>
> 同时，出于安全考虑，配置文件必须是普通文件（regular file）：如果是 symlink/目录/特殊文件，会被拒绝加载。

## 顶层 schema

```json
{
  "version": 1,
  "client": {
    "protocol_version": "2025-06-18",
    "capabilities": {},
    "roots": [
      { "uri": "file:///repo", "name": "workspace" }
    ]
  },
  "servers": {
    "server_name": { "transport": "stdio", "argv": ["bin", "--stdio"] }
  }
}
```

字段说明：

- `version`（必填）：目前只支持 `1`
- `client`（可选）：覆盖 MCP initialize 中的 client 信息
  - `protocol_version`（可选）：非空字符串
  - `capabilities`（可选）：JSON object
  - `roots`（可选）：启用 roots 能力并内建响应 `roots/list`（见下文）
- `servers`（必填但可为空 object）：server 配置字典

## server name 约束

`servers` 的 key（server name）只允许字符：`[a-zA-Z0-9_-]`，且不能为空。

## `servers.<name>` 通用字段

所有 transport 共有字段：

- `transport`（必填）：`"stdio" | "unix" | "streamable_http"`

不同 transport 允许的字段不同；不允许的字段会报错。

## transport=stdio

```json
{
  "transport": "stdio",
  "argv": ["mcp-server-bin", "--stdio"],
  "env": { "KEY": "VALUE" },
  "stdout_log": {
    "path": "./.mcp-kit/logs/mcp/server.stdout.log",
    "max_bytes_per_part": 1048576,
    "max_parts": 32
  }
}
```

字段：

- `argv`（必填）：非空数组；每项必须非空字符串
- `inherit_env`（可选，默认 `false`）：是否继承当前进程环境变量。若为 `false`，会清空子进程环境，只透传少量基础变量并再注入 `env`，以降低宿主 secrets 泄露风险：
  - 基线 env 白名单（当前实现）：`PATH`、`HOME`、`USERPROFILE`、`TMPDIR`、`TEMP`、`TMP`、`SystemRoot`、`SYSTEMROOT`
  - 兼容性提示：如果你的 server 依赖其它变量（如 `LANG/LC_*`、`XDG_*`、证书/代理相关变量等），请显式写入 `servers.<name>.env`（或保持 `inherit_env=true`）
- `env`（可选）：KV 字典，注入到 child process
- `stdout_log`（可选）：stdout 旋转落盘（便于排查协议输出）
  - `path`（必填）：可为相对路径（相对当前 config 文件所在目录解析；默认发现的 config 位于 `--root` 下时，与“相对 `--root`”等价）
    - 额外约束：`path` 不允许包含 `..` 段（防止路径穿越）
    - 额外约束：默认要求 `path` 位于当前 config 文件所在目录之下（需要写到该目录外时，CLI：`--allow-stdout-log-outside-root`；代码：`Manager::with_allow_stdout_log_outside_root(true)`）
    - 额外约束：出于安全考虑，`path` 不允许包含任何 symlink 路径组件（含父目录/目标文件）
  - `max_bytes_per_part`（可选，默认 1MiB，最小 1）
  - `max_parts`（可选，默认 32，最小 1；`0` 表示不做保留上限：无限保留；手动构造 Rust `StdoutLogConfig` 时也接受 `Some(0)`，并按 unlimited 处理）

stdout_log 的旋转文件命名/保留策略见 [`日志与观测`](logging.md)。

安全：

- 默认 `TrustMode::Untrusted` 会拒绝 `stdio`（避免不可信仓库导致本地执行）。需要显式 `--trust --yes-trust` 或 `Manager::with_trust_mode(Trusted)`。

## transport=unix

```json
{ "transport": "unix", "unix_path": "/tmp/mcp.sock" }
```

字段：

- `unix_path`（必填）：可为相对路径（相对当前 config 文件所在目录解析；默认发现的 config 位于 `--root` 下时，与“相对 `--root`”等价）

约束：

- `unix_path` 不能包含 `..` path segment；相对路径只能落在当前 config 文件所在目录内部，不允许静默逃逸到该目录外
- 不支持 `argv/env/stdout_log`（仅用于连接已存在的 unix socket）

安全：

- 默认 `TrustMode::Untrusted` 会拒绝 `unix`（避免不可信仓库连接本地敏感 socket）。需要显式信任。

## transport=streamable_http

```json
{
  "transport": "streamable_http",
  "url": "https://example.com/mcp",
  "http_headers": { "X-Client": "my-app" },
  "bearer_token_secret": "secret://env/MCP_TOKEN",
  "secret_http_headers": { "X-Api-Key": "secret://env/MCP_API_KEY" }
}
```

字段：

- `url`（可选）：远程 MCP server URL（同时用于 SSE 与 POST）
- `sse_url` + `http_url`（可选）：分离的 SSE URL 与 POST URL（两者必须同时设置；不能与 `url` 同时出现）
- `http_headers`（可选）：静态 header
- `bearer_token_secret`（可选）：通过 `secret-kit` 解析 secret spec，并注入 `Authorization: Bearer ...`
- `secret_http_headers`（可选）：通过 `secret-kit` 解析每个 header 的 secret spec

兼容性：

- 仍接受 legacy `bearer_token_env_var` / `env_http_headers`
- 加载后会自动规范化成 `secret://env/...` 的 canonical secret spec 语义

约束：

- 不支持 `argv/unix_path/env/stdout_log`
- Trusted 模式下，`url` / `sse_url` / `http_url` / `http_headers` 只允许 `${MCP_ROOT}` / `${CLAUDE_PLUGIN_ROOT}` 两种 root placeholder；不再允许 `${ENV}` 直接注入 transport 配置

安全（默认 Untrusted）：

- 允许连接远程 `https` 且 host 看起来是公网域名的 `url`（默认拒绝 `localhost` / `localhost.localdomain` / `*.localhost`、`*.local` / `*.localdomain`、**单标签 host**、私网/loopback IP 字面量，以及 DNS 解析到非公网 IP 的 hostname）
- 拒绝发送敏感 header：`Authorization/Cookie/Proxy-Authorization`
- 拒绝解析 `bearer_token_secret` / `secret_http_headers`（包括 legacy env aliases）

详见 [`安全模型`](security.md)。

> 注意：即使你配置了 `allowed_hosts` / CLI `--allow-host`，它也不会覆盖 `localhost/localdomain/单标签 host` 的默认拦截；其中 `allow_localhost` / `--allow-localhost` 只会放开 `localhost` / `localhost.localdomain` / `*.localhost`。如果要连接 `*.local`、`*.localdomain` 或单标签 host，只能直接使用 `Trusted`。

streamable_http 的具体 HTTP 形态（SSE + POST、`mcp-session-id`、回包为 SSE 的场景）见 [`streamable_http 传输详解`](streamable_http.md)。

## client.roots 与 `roots/list`

当配置了 `client.roots`（或在代码里用 `Manager::with_roots(...)`）：

- 会自动在 initialize 中声明 `capabilities.roots`
- 会内建响应 server→client request：`roots/list`（返回你配置的 roots）

`Root` 结构：

```json
{ "uri": "file:///repo", "name": "workspace" }
```

其中：

- `uri` 必须非空
- `name` 可选；若存在必须非空
