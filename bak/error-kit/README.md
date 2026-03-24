# error-kit

`error-kit` 已经不是当前活跃 workspace 成员。

原先放在这里的通用结构化文本原语已经迁移为更窄的领域边界：

- `structured-text-kit`
- `structured-text-protocol`

`i18n-kit`、`secret-kit` 等当前活跃 crate 应该依赖新的文本原语，而不是继续依赖这个旧名字。

如果你的目标是“错误领域”，建议重新从错误语义出发设计新的 `error-kit`，并让它单向依赖 `structured-text-kit`，而不是反过来把通用文本原语塞进错误工具名下。

仓库里的 `bak/error-kit/` 目录目前只应被视为迁移遗留物，而不是当前推荐依赖目标。
