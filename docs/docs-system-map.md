# omne_foundation Docs System

## Start Here

- 外部概览：`../README.md`
- 根入口地图：`../AGENTS.md`
- workspace 顶层结构：`../ARCHITECTURE.md`
- 文档地图：`README.md`
- foundation 定义：`定义/foundation.md`
- prompt 领域定位：`定义/prompt领域定位.md`
- 跨仓库复用基建地图：`定义/跨仓库复用基建地图.md`
- 工程规范索引：`规范/README.md`
- crate 索引：`crates/README.md`

## 记录系统规则

- `AGENTS.md` 只做短地图，不承担百科全书职责。
- `ARCHITECTURE.md` 负责 workspace 级分层、依赖方向和高层边界。
- `docs/` 才是受版本控制的事实记录系统。
- workspace 级规则放在 `docs/规范/`。
- crate 索引放在 `docs/crates/README.md`。
- crate 级事实放在 `crates/<crate>/README.md`。
- 兼容入口、生成产物和历史缓存不作为事实来源。

## 目录职责

- `定义/`
  - `foundation.md`：定义什么属于 foundation，什么不属于。
  - `prompt领域定位.md`：记录 prompt 基建是否成立、成立到哪一层，以及不该抽象的部分。
  - `跨仓库复用基建地图.md`：记录哪些能力应放在 `omne_foundation`、`omne-runtime`、独立 harness，哪些应留在业务仓。
- `规范/`
  - workspace 级版本、兼容、提交、文档系统与质量门禁规则。
- `crates/`
  - `README.md`：各 foundation crate 的正式说明。
- `docs/crates/`
  - crate 索引。

## 新鲜度规则

- 跨仓边界变化时，先更新 `定义/跨仓库复用基建地图.md`。
- foundation 定义变化时，更新 `定义/foundation.md`。
- crate 归属或职责变化时，更新对应 `crates/<crate>/README.md`。
- `README.md`、`AGENTS.md`、`docs/` 入口之间必须互相指向。
- 可机械检查的文档结构优先接入 `scripts/check-workspace.sh`。
