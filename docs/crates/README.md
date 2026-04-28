# Crate 文档索引

这一层只回答一个问题：`omne_foundation` 里的每个 crate 分别负责什么。

当前活跃 crate 的说明已经迁到各自的 `README.md`。

## 索引

- [`structured-text-kit`](../../crates/structured-text-kit/README.md)
- [`structured-text-protocol`](../../crates/structured-text-protocol/README.md)
- [`error-kit`](../../crates/error-kit/README.md)
- [`error-protocol`](../../crates/error-protocol/README.md)
- [`github-kit`](../../crates/github-kit/README.md)
- [`log-kit`](../../crates/log-kit/README.md)
- [`i18n-kit`](../../crates/i18n-kit/README.md)
- [`config-kit`](../../crates/config-kit/README.md)
- [`text-assets-kit`](../../crates/text-assets-kit/README.md)
- [`i18n-runtime-kit`](../../crates/i18n-runtime-kit/README.md)
- [`prompt-kit`](../../crates/prompt-kit/README.md)
- [`secret-kit`](../../crates/secret-kit/README.md)
- [`http-auth-kit`](../../crates/http-auth-kit/README.md)
- [`http-kit`](../../crates/http-kit/README.md)
- [`mcp-jsonrpc`](../../crates/mcp-jsonrpc/README.md)
- [`mcp-kit`](../../crates/mcp-kit/README.md)
- [`notify-kit`](../../crates/notify-kit/README.md)
- [`policy-meta`](../../crates/policy-meta/README.md)

## 阅读顺序

- 想从结构化文本语义开始：
  - `structured-text-kit` -> `structured-text-protocol` -> `error-kit` -> `error-protocol` -> `log-kit` -> `i18n-kit`
- 想从配置与运行时输入开始：
  - `config-kit`
  - `text-assets-kit` -> `i18n-runtime-kit` -> `i18n-kit`
  - [`../定义/prompt领域定位.md`](../定义/prompt领域定位.md) -> `text-assets-kit` -> `prompt-kit`
  - `secret-kit`
- 想从跨仓库策略契约开始：
  - `policy-meta`
- 想从网络边界和协议连接开始：
  - `http-kit` -> `http-auth-kit` -> `github-kit`
  - `http-kit` -> `mcp-jsonrpc` -> `mcp-kit`
- 想从 HTTP foundation 开始：
  - `http-kit` -> `http-auth-kit` -> `github-kit`
- 想从日志与观测语义开始：
  - `structured-text-kit` -> `log-kit`
- 想从通知域开始：
  - `structured-text-kit` -> `log-kit` -> `http-kit` -> `notify-kit`
