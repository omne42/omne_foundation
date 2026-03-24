# error-kit

这个文件保留为历史说明。

`error-kit` 已经不是当前活跃 workspace 成员。原先放在这里的通用结构化文本原语已经拆分为更窄的结构化文本领域：

- [`structured-text-kit`](./structured-text-kit.md)
- [`structured-text-protocol`](./structured-text-protocol.md)

这样做的原因是：

- `error-kit` 这个名字属于“错误工具”领域
- `i18n-kit` 和 `secret-kit` 实际依赖的是更通用但也更窄的“结构化用户文本”原语
- 用 `message` 给这个原语命名过于宽泛，容易和 IM、内部通信、协议消息混淆

仓库里保留的 `bak/error-kit/` 目录现在只是迁移遗留物：

- README 只保留墓碑说明
- crate 入口会直接报停用错误
- 旧实现已经移出 `crates/`，不再伪装成活跃 crate

如果你需要新的错误领域能力，应该重新定义错误语义，再单向依赖 `structured-text-kit`，而不是继续把通用文本原语挂在 `error-kit` 名下。
