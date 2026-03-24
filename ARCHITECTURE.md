# omne_foundation Architecture

`omne_foundation` 是一个 Rust workspace，目标不是提供单一产品，而是沉淀可复用的基础能力 crate。

这个文件只回答顶层问题：

- workspace 分成哪些领域层
- crate 之间的主要依赖方向是什么
- 应该去哪里找更具体的事实来源

详细的 crate 说明已经拆到 [`docs/crates/`](./docs/crates/README.md)。

## 读图规则

下面的箭头统一表示：

```text
A -> B   表示 A 依赖 B
```

## 顶层分层

### 1. 结构化文本语义层

- `structured-text-kit`
- `structured-text-protocol`
- `i18n-kit`

这一层处理“用户可见结构化文本是什么，以及如何跨边界表示它”：

- `structured-text-kit` 定义 `StructuredText` / `CatalogText`
- `structured-text-protocol` 把结构化文本映射到 JSON Schema / TypeScript DTO
- `i18n-kit` 按 locale/catalog/template 把结构化文本渲染成最终文本

这里有一个需要显式说明的边界选择：

- 这里刻意不用更泛化的 “message” 概念。
- `structured-text-kit` 只建模“catalog-backed 或 freeform 的用户可见结构化文本”。
- 它不是 IM 消息、进程间通信消息、事件总线消息，也不是通用消息系统。

### 2. 运行时输入层

- `runtime-assets-kit`
- `secret-kit`

这一层处理“运行时如何安全地拿到输入”：

- i18n / prompts 等文本资源如何 bootstrap、落盘、回滚、懒加载
- secret 如何通过统一 `secret://` 规范解析

### 3. 传输与会话层

- `mcp-jsonrpc`
- `mcp-kit`

这一层处理“如何连接协议端点并管理会话”：

- JSON-RPC transport
- MCP config / initialize / manager / session / security model

### 4. 通知层

- `notify-kit`

这一层独立处理“如何把统一事件投递到外部通知渠道”。

## 主要依赖方向

当前 workspace 内部可总结成下面这张简图：

```text
structured-text-protocol -> structured-text-kit
i18n-kit              -> structured-text-kit
secret-kit            -> structured-text-kit

runtime-assets-kit    -> i18n-kit        (feature = "i18n")

mcp-kit              -> mcp-jsonrpc

notify-kit           -> (no workspace crate dependency)
```

补充说明：

- `runtime-assets-kit` 依赖 `i18n-kit`，不是反过来。
- `notify-kit` 当前是独立域，不依赖 workspace 内其他 crate。
- `mcp-jsonrpc` 是 transport 层，`mcp-kit` 在其上增加 MCP 语义和配置管理。
- `i18n-kit` 和 `secret-kit` 依赖的是结构化文本原语，不是错误处理流程。

## 边界原则

这个 workspace 目前遵循几条简单边界原则：

- 一个 crate 只承载一个稳定领域，不把上层应用语义硬塞进 foundation。
- 协议传输、结构化文本语义、资源加载、secret 解析、通知投递分开建模。
- 能由上层应用决定的策略，不下沉到基础 crate。
- 约束优先放在边界处，crate 内部实现保持足够自由。

## 记录系统

workspace 级文档现在按“渐进式披露”组织：

- [`docs/README.md`](./docs/README.md)
  - 文档地图，先看这里
- `docs/规范/<topic>.md`
  - workspace 级版本、兼容、发布等治理规则
- [`docs/crates/README.md`](./docs/crates/README.md)
  - crate 索引
- `docs/crates/<crate>.md`
  - 每个 crate 的领域、边界、范围、结构设计
- `crates/mcp-kit/docs/`
  - `mcp-kit` 的详细专题文档
- `crates/notify-kit/docs/`
  - `notify-kit` 的详细专题文档

## 文档维护约束

为了避免文档重新退化成“一个巨大的总览文件”，根级文档按下面的规则维护：

- `ARCHITECTURE.md` 只保留 workspace 级地图，不堆实现细节。
- `docs/README.md` 只做入口导航，不重复 crate 细节。
- 版本、兼容、发布等规则写入 `docs/规范/<topic>.md`。
- crate 事实写入对应的 `docs/crates/<crate>.md`。
- crate 专题细节优先放到 crate 自己的 `docs/` 或 `README.md`。
