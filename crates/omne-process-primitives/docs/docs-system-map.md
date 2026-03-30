# omne-process-primitives Docs System

## Start Here

- 外部概览：`../README.md`
- 执行者地图：`../AGENTS.md`
- 边界说明：`architecture/system-boundaries.md`
- 源码布局：`architecture/source-layout.md`
- workspace 边界：`../../../docs/workspace-crate-boundaries.md`

## 文档分工

- `README.md`
  - 对外概览和最小验证。
- `AGENTS.md`
  - 短地图。
- `docs/architecture/system-boundaries.md`
  - 宿主机命令与进程原语边界。
- `docs/architecture/source-layout.md`
  - 源码职责说明。

## 维护规则

- 如果逻辑开始携带 allowlist、超时或产品错误映射，它通常不该继续留在这里。
- 新增宿主命令或进程树原语时，同步更新边界和布局文档。

## Verify

- `cargo test -p omne-process-primitives`
- `../../../scripts/check-docs-system.sh`
