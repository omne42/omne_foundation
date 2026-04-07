# secret-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`secret-kit` 负责 secret 规范、解析和安全持有。

它把 secret 输入统一到 `secret://` 语法下，并在读取、命令执行、JSON 提取和内存持有阶段尽量减少泄露面。

## 边界

负责：

- `secret://` 规范解析
- 多 provider secret 读取
- `SecretString` 安全持有与 zeroize
- 文件读取约束
- 外部命令执行、超时和清理
- JSON secret 字段提取

不负责：

- 业务凭证申请流程
- 长期托管服务本身
- 上层 secret 生命周期编排

## 范围

覆盖：

- `env`
- `file`
- `vault`
- `aws-sm`
- `gcp-sm`
- `azure-kv`

同时覆盖：

- CLI provider 调用
- 内建 provider 的系统目录级 PATH 发现、子进程 PATH 裁剪与显式绝对路径 override
- JSON 字段提取
- 输出大小与命令超时限制
- `SECRET_COMMAND_TIMEOUT_MS` / `SECRET_COMMAND_TIMEOUT_SECS` 的显式 command-env 超时调优入口
- 进程树清理

不覆盖：

- 内嵌各云 SDK 的 provider 客户端

## 结构设计

- `src/lib.rs`
  - `SecretString`
  - `SecretError`
  - 运行时 trait
  - 默认 resolver 主体
- `src/spec.rs`
  - `secret://` 解析、provider 分派、命令构建
- `src/file.rs`
  - 受限 secret 文件读取与 symlink 约束
- `src/command.rs`
  - 外部命令执行、trusted PATH 解析、stdout/stderr 限制、进程清理
- `src/json.rs`
  - JSON secret 字段提取与中间值 zeroize

## 与其他 crate 的关系

- 依赖 [`structured-text-kit`](../structured-text-kit/README.md) 表达结构化错误文本
- 依赖 [`error-kit`](../error-kit/README.md) 提供稳定错误码、类别和重试语义映射
- 与 `text-assets-kit`、`i18n-runtime-kit`、`prompt-kit`、`mcp-kit`、`notify-kit` 没有强耦合

## CLI 发现边界

- 内建 `vault` / `aws` / `gcloud` / `az` provider 不会信任任意 ambient `PATH` 项。
- 默认只会在 ambient allowlist 里保留下来的系统目录级 `PATH` 项中搜索内建 CLI。
- 显式 `command_env_pairs()` 不能把新的 `PATH` 搜索目录注入进来。
- 如果调用方需要使用工作区、自定义 shim 或用户目录里的二进制，应通过 `resolve_command_program(...)` 提供绝对路径 override，而不是依赖 ambient `PATH`。
