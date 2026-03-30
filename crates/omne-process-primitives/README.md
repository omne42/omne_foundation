# omne-process-primitives

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`omne-process-primitives` 负责低层进程与 host command 原语。

它只提供更高层 crate 共享的命令探测、host command 执行和 process-tree cleanup building blocks，不承载产品级 secret 或工具编排语义。

## 边界

负责：

- command path resolution
- host command 执行与输出采集
- process-tree cleanup 原语

不负责：

- secret provider 语义
- 产品级安装/执行流程

## 与其他 crate 的关系

- 当前被 `secret-kit` 复用
- 保持为 `omne_foundation` 内部底层原语，不再从外部 `omne-runtime` workspace 反向依赖
