# prompt-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`prompt-kit` 当前只负责 prompt 目录的 runtime adapter 边界。

它解决的问题不是“prompt 文本应该长什么样”，也不是“prompt 如何版本化、指纹化或缓存”，而是“一组 prompt 文本资源如何安全 bootstrap、加载，并通过惰性句柄对外提供稳定访问”。

这一层是窄适配，不是当前 cross-repo canonical 的 prompt foundation。更高层的 prompt 领域判断见 [`../../docs/定义/prompt领域定位.md`](../../docs/定义/prompt领域定位.md)。

## 边界

负责：

- prompt 目录 bootstrap
- prompt 目录快照加载
- `LazyPromptDirectory`
- prompt 初始化错误与回滚错误包装

不负责：

- system prompt 组装
- prompt bundle typing / schema attachment / fingerprint / cache key
- prompt 模板语义本身
- 通用文本资源/data root/secure fs 基础能力
- i18n 语义
- 远程 prompt 分发

## 范围

覆盖：

- `bootstrap_prompt_directory(...)`
- `LazyPromptDirectory`
- `PromptDirectoryError`
- `PromptBootstrapCleanupError`

不覆盖：

- prompt 渲染 DSL
- prompt 版本管理、身份建模与远程同步

## 结构设计

- `src/prompts.rs`
  - prompt bootstrap、惰性目录句柄与错误包装
- 共享懒初始化并发控制与 best-effort bootstrap/rollback 流程
  - 由 [`text-assets-kit`](../text-assets-kit/README.md) 提供通用原语，`prompt-kit` 只保留 prompt 域错误映射

## 当前定位

- 代码上它本质是建立在 [`text-assets-kit`](../text-assets-kit/README.md) 之上的薄封装。
- 当前跨仓库已证明的 prompt 复用面，并不主要是“prompt 目录加载”。
- 因此当前不应该继续往 `prompt-kit` 塞入模板 DSL、版本管理、schema 绑定、fingerprint 或 cache-key 语义。

如果未来出现第二个以上稳定消费者，需要复用的是更高层的 prompt bundle identity，而不是继续放大这个目录适配层。

## 与其他 crate 的关系

- 建立在 [`text-assets-kit`](../text-assets-kit/README.md) 之上，复用通用文本资源与 bootstrap 能力
- manifest、目录快照和文本资源类型由 [`text-assets-kit`](../text-assets-kit/README.md) 直接提供，不再从 `prompt-kit` 根导出
- 不把 prompt 业务语义回塞到 `text-assets-kit`
