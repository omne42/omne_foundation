# 源码布局

## 入口

- `src/lib.rs`
  - crate 入口与公开导出。

## 主要模块

- `src/cap_root.rs`
  - root 打开、目录访问和 capability 风格路径操作。
- `src/platform_open.rs`
  - no-follow 打开与 symlink/reparse 相关辅助。
- `src/read_limited.rs`
  - bounded read 与 UTF-8 文本读取。
- `src/atomic_write.rs`
  - staged atomic file/directory write、replace 与替换逻辑。
- `src/advisory_lock.rs`
  - advisory file lock。
- `src/path_identity.rs`
  - 文件系统大小写敏感性识别。

## 布局规则

- 文件名必须直接表达原语职责。
- 若新增原语跨越多个现有文件职责，应先拆清边界再实现。
