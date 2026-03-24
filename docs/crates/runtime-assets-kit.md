# runtime-assets-kit

源码入口：[`crates/runtime-assets-kit/src/lib.rs`](../../crates/runtime-assets-kit/src/lib.rs)

## 领域

`runtime-assets-kit` 负责运行时文本资源管理。

它解决的问题不是“资源语义是什么”，而是“资源如何在受控目录中被安全地 bootstrap、落盘、加载、回滚和懒初始化”。

## 边界

负责：

- data root 决议
- 资源路径规范化
- 文本资源与资源清单
- 受限文件系统访问
- bootstrap 事务、失败回滚、懒初始化
- `i18n` 与 `prompts` 两类资源接线

不负责：

- 资源内容的业务语义
- 复杂模板语言
- 远程下载和同步
- secret 解析

## 范围

覆盖：

- `.omne_data` / 环境变量 / 显式路径 data root
- 文本文件大小和目录总大小限制
- 目录遍历、symlink、越界路径约束
- i18n catalog bootstrap
- prompt 目录 bootstrap

不覆盖：

- 二进制资源管理
- 通用包管理器

## 结构设计

- `src/data_root.rs`
  - data root scope、路径优先级和根目录决议
- `src/resource_path.rs`
  - 资源相对路径规范化与 identity 规则
- `src/secure_fs.rs`
  - 安全文件系统访问收口
- `src/text_resource.rs`
  - 单个文本资源与资源清单
- `src/text_directory.rs`
  - 已加载文本目录快照
- `src/resource_bootstrap.rs`
  - bootstrap、创建报告与失败回滚
- `src/lazy_state.rs`
  - 懒初始化并发控制
- `src/i18n.rs`
  - 基于 manifest bootstrap `i18n-kit`
- `src/prompts.rs`
  - 基于 manifest bootstrap prompt 目录

## 与其他 crate 的关系

- 可选依赖 `i18n-kit`
- 不反向定义 `i18n-kit` 的语义，只负责把资源安全地交给它
