`AGENTS.md` 只保留根入口和硬边界，不承担总手册职责。更具体的事实请进入 `docs/` 做渐进式披露。

## 入口地图

- 仓库外部概览：`README.md`
- 仓库级文档地图：`docs/README.md`
- 文档系统地图：`docs/docs-system-map.md`
- workspace 顶层结构：`ARCHITECTURE.md`
- 文档系统规范：`docs/规范/文档系统.md`
- 工程规范索引：`docs/规范/README.md`
- crate 索引：`docs/crates/README.md`
- crate 级事实：各 crate 自己的 `README.md`
- crate 专题文档：各 crate 自己的 `docs/`

## 硬边界

- `AGENTS.md` 只做地图，不堆实现细节。
- `ARCHITECTURE.md` 只保留 workspace 级分层、依赖方向和记录系统入口。
- `docs/README.md` 只保留稳定入口，不重复 crate 细节。
- workspace 级治理规则写到 `docs/规范/`。
- crate 级事实写到各 crate 的 `README.md`。
- `docs/crates/` 只保留索引，不再承载正文。

## 维护方式

- 重要约束优先沉淀到仓库内可链接文档，不留在聊天记录里。
- 能机械检查的文档结构，优先接到 `scripts/check-workspace.sh`。
- 如果 `AGENTS.md` 开始变长，应该继续把细节下沉到 `docs/`。

## 最低检查

- `scripts/check-workspace.sh docs-system`
- `scripts/check-workspace.sh local`
- `scripts/check-workspace.sh ci`
