# text-assets-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`text-assets-kit` 负责通用文本资源边界与文件系统适配。

它解决的问题不是“文本资源的业务语义是什么”，而是“文本资源如何在受控目录中被安全地 bootstrap、落盘、扫描、读取和回滚”。

## 边界

负责：

- data root 决议
- 资源路径规范化与 identity 规则
- 文本资源清单与目录快照
- 受限文件系统访问
- bootstrap 事务、失败回滚与跨进程锁
- 文本目录遍历与树扫描

不负责：

- locale 选择、catalog fallback 和 structured text 渲染
- runtime i18n catalog 句柄
- prompt 业务语义
- 远程下载和同步
- secret 解析

## 范围

覆盖：

- 中性默认 data root（`./.text_assets`、`TEXT_ASSETS_DIR`）以及调用方显式覆写后的 data root
- 文本文件大小和目录总大小限制
- 目录遍历、symlink、越界路径约束
- 通用文本资源 manifest bootstrap
- 已加载文本目录快照与树扫描
- `materialize_resource_root(...)`
- `scan_text_directory(...)`

不覆盖：

- i18n catalog 语义校验
- prompt 目录的惰性初始化句柄
- 二进制资源管理

## 结构设计

- `src/data_root.rs`
  - data root scope、路径优先级和根目录决议
- `src/resource_path.rs`
  - 资源相对路径规范化、identity 规则与 resource root 规范化
- `src/secure_fs.rs`
  - 安全文件系统访问收口与目录扫描
- `src/text_resource.rs`
  - 单个文本资源与资源清单
- `src/text_directory.rs`
  - 已加载文本目录快照
- `src/text_tree_scan.rs`
  - 通用文本目录树遍历
- `src/resource_bootstrap.rs`
  - bootstrap、创建报告与失败回滚
- `src/bootstrap_lock.rs`
  - bootstrap 事务锁与跨进程协调

## 与其他 crate 的关系

- 当前不依赖 `omne_foundation` 内其他 crate
- 被 [`i18n-runtime-kit`](../i18n-runtime-kit/README.md) 和 [`prompt-kit`](../prompt-kit/README.md) 作为更高层 runtime adapter 复用
- 由它统一承载通用文本资源 root 规范化与目录扫描，不再让上层 runtime adapter 直接下钻 `omne-fs-primitives`
- 刻意不反向承载 i18n 或 prompt 语义，避免把通用文本资源边界重新做宽
