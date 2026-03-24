# secret-kit

源码入口：[`crates/secret-kit/src/lib.rs`](../../crates/secret-kit/src/lib.rs)

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
- JSON 字段提取
- 输出大小与命令超时限制
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
  - 外部命令执行、超时、stdout/stderr 限制、进程清理
- `src/json.rs`
  - JSON secret 字段提取与中间值 zeroize

## 与其他 crate 的关系

- 依赖 `structured-text-kit` 表达结构化错误文本
- 与 `runtime-assets-kit`、`mcp-kit`、`notify-kit` 没有强耦合
