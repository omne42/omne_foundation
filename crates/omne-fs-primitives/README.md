# omne-fs-primitives

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`omne-fs-primitives` 负责低层文件系统原语。

它只提供更高层 crate 共享的 no-follow、cap-style root walking、bounded read 和 atomic write building blocks，不承载业务语义。

## 边界

负责：

- root materialization 与 capability-style 目录访问
- no-follow regular file open
- bounded file read
- advisory lock
- atomic file / directory write

不负责：

- 产品级配置语义
- 文本资源业务规则
- secret 语义

## 与其他 crate 的关系

- 被 `config-kit`、`text-assets-kit`、`secret-kit` 复用
- 保持为 `omne_foundation` 内部底层原语，不再从外部 `omne-runtime` workspace 反向依赖
