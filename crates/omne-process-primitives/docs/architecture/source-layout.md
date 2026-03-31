# 源码布局

## 入口

- `src/lib.rs`
  - crate 入口、进程树清理原语和跨平台 cleanup 逻辑。

## 主要模块

- `src/host_command.rs`
  - 宿主机命令探测、host command / host recipe 执行、默认 `sudo` 模式推断，以及 `sudo -n` 相关实现。

## 布局规则

- 新增进程/命令原语时，文件名必须直接表达职责。
- 若未来把 Unix 和 Windows cleanup 细分，继续保持平台文件名直观可读。
