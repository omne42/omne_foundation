# prompt-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`prompt-kit` 当前只负责 prompt 目录的 runtime adapter 边界。

它解决的问题不是“prompt 文本应该长什么样”，也不是“prompt 如何版本化、指纹化或缓存”，而是“一组 prompt 文本资源如何安全 bootstrap、加载，并通过窄 runtime 句柄对外提供稳定访问”。

这一层是窄适配，不是当前 cross-repo canonical 的 prompt foundation。更高层的 prompt 领域判断见 [`../../docs/定义/prompt领域定位.md`](../../docs/定义/prompt领域定位.md)。

## 边界

负责：

- prompt 目录 bootstrap
- prompt 目录快照加载
- `PromptDirectoryHandle`
- 兼容层 `LazyPromptDirectory`
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
- `bootstrap_prompt_directory_with_base(...)`
- `PromptDirectoryHandle`
- 兼容层 `LazyPromptDirectory`
- `PromptDirectoryError`
- `PromptBootstrapCleanupError`

不覆盖：

- prompt 渲染 DSL
- prompt 版本管理、身份建模与远程同步

## 结构设计

- `src/prompts.rs`
  - prompt bootstrap、runtime 目录句柄与错误包装
- best-effort bootstrap/rollback 流程
  - 由 [`text-assets-kit`](../text-assets-kit/README.md) 提供通用文本资源原语
- `src/lazy_compat.rs`
  - `LazyPromptDirectory` 专用的私有阻塞 compat shim，不再把通用 blocking lazy primitive 暴露回 foundation crate 边界

## 当前定位

- 代码上它本质是建立在 [`text-assets-kit`](../text-assets-kit/README.md) 之上的薄封装。
- `PromptDirectoryHandle` 是当前推荐的 runtime-facing 共享句柄；`LazyPromptDirectory` 仅保留为已废弃的阻塞式兼容层。
- `LazyPromptDirectory` 对同线程递归初始化、同线程初始化冲突以及可检测的线程级跨线程初始化环路都会返回显式错误，但它仍然是阻塞式兼容层，不应继续扩张成 runtime-facing canonical API。
- 这个阻塞 compat shim 现在收口在 `prompt-kit` 私有模块里；`text-assets-kit` 继续只暴露文本资源 runtime adapter 能力，不再承担通用 blocking lazy public surface。
- 当前跨仓库已证明的 prompt 复用面，并不主要是“prompt 目录加载”。
- 因此当前不应该继续往 `prompt-kit` 塞入模板 DSL、版本管理、schema 绑定、fingerprint 或 cache-key 语义。

如果未来出现第二个以上稳定消费者，需要复用的是更高层的 prompt bundle identity，而不是继续放大这个目录适配层。

## 与其他 crate 的关系

- 建立在 [`text-assets-kit`](../text-assets-kit/README.md) 之上，复用通用文本资源与 bootstrap 能力
- manifest、目录快照和文本资源类型由 [`text-assets-kit`](../text-assets-kit/README.md) 直接提供，不再从 `prompt-kit` 根导出
- 当调用方已经持有稳定 workspace/root 事实时，应优先使用 `bootstrap_prompt_directory_with_base(...)`，而不是继续让相对目录依赖 ambient `current_dir()`
- 不把 prompt 业务语义回塞到 `text-assets-kit`
