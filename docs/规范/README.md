# 规范索引

`docs/规范/` 记录 `omne_foundation` 的 workspace 级工程规范。

这些规范的目标不是重复脚本实现细节，而是把仓库已经在机械执行或长期依赖的规则，整理成可链接、可维护、可审阅的 system of record。

## 从哪里开始

- 想看 agent-first 入口和地图怎么分层：看 [`./文档系统.md`](./文档系统.md)
- 想看版本号、兼容和 public API gate：看 [`./版本与兼容.md`](./版本与兼容.md)
- 想看分支命名和 Conventional Commits：看 [`./提交与分支.md`](./提交与分支.md)
- 想看 changelog 维护规则：看 [`./变更记录.md`](./变更记录.md)
- 想看 githook、workspace checks、发布契约回归 gate 和 asset checks：看 [`./Hook与质量门禁.md`](./Hook与质量门禁.md)

## 设计原则

- 每个规范文件只覆盖一个稳定主题，不写成大一统总手册。
- `AGENTS.md`、`ARCHITECTURE.md`、`docs/README.md` 负责地图；这里负责规则。
- 能机械执行的规则，优先以 hook、脚本、lint 或 CI 落地。
- 文档负责说明边界、目的、例外和人工判断点，不重复实现代码。
- 如果脚本行为发生变化，应同步更新这里对应的规范文件。

## 与自动化的关系

当前 `omne_foundation` 的规范执行主要依赖这些入口：

- [`githooks/pre-commit`](../../githooks/pre-commit)
- [`githooks/commit-msg`](../../githooks/commit-msg)
- [`scripts/pre_commit_check/`](../../scripts/pre_commit_check/)
- [`scripts/version_policy/`](../../scripts/version_policy/)
- [`scripts/workspace_check/`](../../scripts/workspace_check/)
- [`scripts/check-version-policy.py`](../../scripts/check-version-policy.py)
- [`scripts/check-workspace.sh`](../../scripts/check-workspace.sh)

其中：

- `scripts/pre_commit_check/` 是当前 `pre-commit` 的 canonical 实现
- `scripts/version_policy/` 是版本策略的共享核心，供 `pre-commit` 与 `commit-msg` 共用
- `scripts/workspace_check/` 是 workspace 质量检查的共享核心，供 `pre-commit`、手动命令和 CI 共用
- `scripts/check-version-policy.py` 与 `scripts/check-workspace.sh` 现在只是薄入口
- `scripts/pre_commit_check/` 作为共享检查引擎保留可迁移性，支持 `root` / `crate` 两种仓库形态
- `omne_foundation` 当前仓库通过 hook wrapper 把它固定在 `crate` 形态

规范目录不是这些脚本的替代品，而是对它们的解释层。
