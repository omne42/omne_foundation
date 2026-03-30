# omne-fs-primitives Docs System

## Start Here

- 外部概览：`../README.md`
- 执行者地图：`../AGENTS.md`
- 边界说明：`architecture/system-boundaries.md`
- 源码布局：`architecture/source-layout.md`
- workspace 边界：`../../../docs/workspace-crate-boundaries.md`

## 文档分工

- `README.md`
  - 对外概览与最小验证。
- `AGENTS.md`
  - 短地图，不存放完整事实。
- `docs/architecture/system-boundaries.md`
  - 低层文件系统原语的职责边界。
- `docs/architecture/source-layout.md`
  - 各源码文件的职责定位。

## 维护规则

- 如果逻辑开始解释策略、权限或 secret 语义，它通常不该放在这里。
- 新增可复用原语时，同步更新边界和源码布局文档。

## Verify

- `cargo test -p omne-fs-primitives`
- `../../../scripts/check-docs-system.sh`
