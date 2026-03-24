# Crate 文档索引

这一层只回答一个问题：`omne_foundation` 里的每个 crate 分别负责什么。

每个 crate 文档统一包含四个部分：

- 领域
- 边界
- 范围
- 结构设计

## 索引

- [`structured-text-kit`](./structured-text-kit.md)
- [`structured-text-protocol`](./structured-text-protocol.md)
- [`i18n-kit`](./i18n-kit.md)
- [`runtime-assets-kit`](./runtime-assets-kit.md)
- [`secret-kit`](./secret-kit.md)
- [`mcp-jsonrpc`](./mcp-jsonrpc.md)
- [`mcp-kit`](./mcp-kit.md)
- [`notify-kit`](./notify-kit.md)

下面这些文件仍可能存在于 `docs/crates/`，但它们只是历史兼容入口，不代表当前活跃 crate：

- `error-kit.md`
- `error-kit-protocol.md`

## 阅读顺序

- 想从结构化文本语义开始：
  - `structured-text-kit` -> `structured-text-protocol` -> `i18n-kit`
- 想从运行时资源和敏感输入开始：
  - `runtime-assets-kit` -> `secret-kit`
- 想从协议通信开始：
  - `mcp-jsonrpc` -> `mcp-kit`
- 想从通知域开始：
  - `notify-kit`
