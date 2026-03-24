# omne_foundation 文档地图

`docs/` 是 `omne_foundation` 的 workspace 级记录系统。这里不重复 crate 内部 mdBook，也不维护一个巨大的“总说明书”，而是提供稳定入口，把你指向更具体的事实来源。

在 agent-first 语境里，这里不只是给人看的文档目录，也应该被视为 repository-local 的 system of record：

- agent 先从这里获得稳定地图
- 再按链接逐步读取更窄、更具体的事实
- 重要约束应尽量落到仓库内，而不是散落在聊天记录或口头约定里
- `AGENTS.md` 应保持短小，主要承担“地图”职责，而不是充当总手册

## 从哪里开始

- 想先理解什么叫 `foundation`：看 [`./定义/foundation.md`](./定义/foundation.md)
- 想先理解 workspace 顶层结构：看 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- 想先看 workspace 级工程规范索引：看 [`./规范/README.md`](./规范/README.md)
- 想先理解版本和兼容规则：看 [`./规范/版本与兼容.md`](./规范/版本与兼容.md)
- 想知道某个 crate 做什么：看 [`./crates/README.md`](./crates/README.md)
- 想看某个 crate 的领域、边界、范围、结构设计：看 `docs/crates/<crate>.md`
- 想深入 `mcp-kit`：看 [`../crates/mcp-kit/docs/README.md`](../crates/mcp-kit/docs/README.md)
- 想深入 `notify-kit`：看 [`../crates/notify-kit/docs/README.md`](../crates/notify-kit/docs/README.md)

## 目录约定

下面这棵目录树只列稳定入口，不试图枚举完整文件树。

- 兼容入口、历史说明、墓碑文件和 crate 内部专题文档不在这里展开
- 看到未列出的文件时，应先判断它是不是兼容入口或更窄层级的事实来源

```text
ARCHITECTURE.md
docs/
├── README.md
├── 定义/
│   └── foundation.md
├── 规范/
│   ├── README.md
│   ├── Hook与质量门禁.md
│   ├── 变更记录.md
│   ├── 提交与分支.md
│   └── 版本与兼容.md
├── crate-boundaries.md
└── crates/
    ├── README.md
    ├── structured-text-kit.md
    ├── structured-text-protocol.md
    ├── i18n-kit.md
    ├── runtime-assets-kit.md
    ├── secret-kit.md
    ├── mcp-jsonrpc.md
    ├── mcp-kit.md
    └── notify-kit.md
```

## 文档维护规则

- 入口文档只做地图，不堆细节。
- 目录树只维护稳定入口，不追求覆盖所有历史兼容文件或专题细节。
- workspace 级架构事实放在 `ARCHITECTURE.md`。
- workspace 级版本、兼容、发布等治理规则放在 `docs/规范/`。
- crate 级事实放在 `docs/crates/<crate>.md`。
- crate 专题细节优先放到 crate 自己的 `README.md` 或 `docs/`。
- 如果某条事实只影响一个 crate，不要回写到 workspace 总览。
- hook、提交、changelog、质量门禁这类 repo 级流程规则，也应优先沉淀到 `docs/规范/`。

## Agent-first 维护约束

- `docs/` 应保持渐进式披露：先给稳定入口，再指向更窄的事实来源。
- 不要把 `docs/` 写成超大说明书；agent 需要地图，而不是失控的上下文洪水。
- 新增稳定约束、边界或决策时，优先写进仓库内可链接文档，而不是留在外部讨论里。
- 能机械检查的知识结构，尽量通过 lint、脚本或 CI 约束保持新鲜度和可发现性。
