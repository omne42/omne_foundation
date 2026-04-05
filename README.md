# omne_foundation

`omne_foundation` 是一个 Rust workspace，用来沉淀跨仓复用、但仍偏应用侧的 foundation 能力。

它当前承载的方向包括：

- 通用配置输入层：`config-kit`（格式识别、有界读取、strict allowed-format typed parse）
- 通用 HTTP 出站层：`http-kit`
- GitHub API client foundation：`github-kit`
- 结构化文本与 i18n：`structured-text-kit`、`structured-text-protocol`、`i18n-kit`、`i18n-runtime-kit`
- prompt / text assets / secret 输入层：`prompt-kit`、`text-assets-kit`、`secret-kit`
- MCP 与通知：`mcp-jsonrpc`、`mcp-kit`、`notify-kit`
- 跨仓共享策略元契约：`policy-meta`

跨仓边界上，本仓库现在明确把 `omne-runtime` 的系统原语视为外部 canonical source：

- workspace manifest 通过 git pinned 依赖接入 `omne-runtime` crate
- 不再通过 `../omne-runtime/...` 这类 sibling path 把两个仓库耦合成隐式单一工作区

它不负责：

- 低层 runtime primitives
- execution gateway / sandbox orchestration
- 具体产品仓库自己的业务数据流和策略执行语义

## 文档入口

这个 workspace 采用 “短入口 + 渐进式披露” 的文档系统。先看这些文件：

- `AGENTS.md`
- `docs/README.md`
- `docs/docs-system-map.md`
- `docs/定义/foundation.md`
- `docs/定义/跨仓库复用基建地图.md`
- `ARCHITECTURE.md`

## 最低验证

```bash
./scripts/check-workspace.sh docs-system
./scripts/check-workspace.sh local
```
