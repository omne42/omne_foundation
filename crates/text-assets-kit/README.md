# text-assets-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`text-assets-kit` 负责通用文本资源边界与文件系统适配。

它解决的问题不是“文本资源的业务语义是什么”，而是“文本资源如何在受控目录中被安全地 bootstrap、落盘、扫描、读取和回滚”。

它内部仍保留共享的阻塞式 lazy-init 兼容原语，供更高层 crate 在迁移路径里复用；但这不是推荐直接暴露给 async runtime 边界的 canonical API。

## 边界

负责：

- data root 决议
- 资源路径规范化与 identity 规则
- 文本资源清单与目录快照
- 受限文件系统访问
- bootstrap 并发串行化、同次尝试失败后的 best-effort rollback 与跨进程锁
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
- 显式 base 驱动的 root/data-root 解析（`materialize_resource_root_with_base(...)`、`resolve_data_root_with_base(...)`、`ensure_data_root_with_base(...)`；相对 resource root、`data_dir` 和 `TEXT_ASSETS_DIR` override 都会先锚到传入 base）
- 显式 base 驱动的高层目录/manifest 入口（`TextDirectory::load_with_base(...)`、`TextDirectory::load_resource_files_with_base(...)`、`bootstrap_text_resources_then_load_with_base(...)`、`bootstrap_text_resources_with_report_with_base(...)`、`scan_text_directory_with_base(...)`）
- 已降级为 compatibility-only ambient 入口（`materialize_resource_root(...)`、`resolve_data_root(...)`、`ensure_data_root(...)`）
- 文本文件大小和目录总大小限制
- 目录遍历、symlink、越界路径约束
- 通用文本资源 manifest bootstrap
- 同一 root 的 bootstrap/load 尝试串行化，以及同次尝试后续失败时的 best-effort 清理
- 已加载文本目录快照与树扫描
- 共享 runtime handle 原语（`SharedRuntimeHandle<T>`），供更高层 runtime adapter 复用“热切换 snapshot 句柄”而不重复实现同一套 `RwLock<Option<Arc<_>>>`
- `scan_text_directory(...)`

不覆盖：

- i18n catalog 语义校验
- prompt 或 i18n 的 runtime 句柄策略
- 二进制资源管理

## 结构设计

- `src/data_root.rs`
  - data root scope、路径优先级和根目录决议；显式 base variants 是 canonical 边界，相对 `data_dir` / `TEXT_ASSETS_DIR` override 会先锚到 base，ambient `current_dir()` 入口只作为 compatibility shim 保留
- `src/resource_path.rs`
  - 资源相对路径规范化、identity 规则与 resource root 规范化；显式 base 是 canonical 入口，ambient compatibility helper 仅保留兼容语义
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
  - bootstrap 并发串行化与跨进程协调；仅作为低层兼容原语保留
- `src/lazy_value.rs`
  - 仅供兼容层复用的阻塞式 lazy-init 原语；模块和类型本体都已显式标记为 deprecated compatibility shim，同线程重入、同线程 in-flight 初始化冲突和可检测的线程级跨线程等待环路都会显式报错，不作为 async runtime-facing canonical 边界

## bootstrap/rollback 语义

- `text-assets-kit` 会串行化同一 resource root 上的并发 bootstrap 尝试，避免一个尝试的 rollback 与另一个尝试的 load 互相踩到。
- 如果同一次 bootstrap 尝试里后续步骤失败，它会按创建报告对本次新建的文件/目录做 best-effort rollback。
- 这不是 crash-safe 或断电恢复事务：如果进程在 bootstrap 写入后异常退出，已创建的文件可能仍然留在磁盘上。
- crate root 的 canonical 入口仍然是 `bootstrap_text_resources_then_load(...)` / `bootstrap_text_resources_with_report(...)`；当调用方已经持有稳定 workspace/root 事实时，应优先切到对应的 `*_with_base(...)` 版本。`bootstrap_lock` 模块只保留给确实需要低层协调的兼容调用方，默认不建议直接依赖。

## 与其他 crate 的关系

- 当前不依赖 `omne_foundation` 内其他 crate
- 被 [`i18n-runtime-kit`](../i18n-runtime-kit/README.md) 和 [`prompt-kit`](../prompt-kit/README.md) 作为更高层 runtime adapter 复用
- 由它统一承载通用文本资源 root 规范化与目录扫描，不再让上层 runtime adapter 直接下钻 `omne-fs-primitives`
- 当调用方已经持有 workspace/root 事实时，应优先使用 `*_with_base(...)` 入口；`materialize_resource_root(...)`、`resolve_data_root(...)`、`ensure_data_root(...)` 现在只作为 compatibility shim 保留，不再视为 canonical foundation 边界
- 刻意不反向承载 i18n 或 prompt 语义，避免把通用文本资源边界重新做宽
