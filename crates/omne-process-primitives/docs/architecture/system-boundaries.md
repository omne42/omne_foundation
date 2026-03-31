# 系统边界

## 目标

`omne-process-primitives` 提供无策略的宿主机命令与进程生命周期原语，供上层 runtime 和 domain caller 复用。

## 负责什么

- 探测命令是否存在和是否可执行。
- 对宿主命令 request/recipe 维持 `OsStr` / `OsString` 边界，不把 argv/env 先强制收窄成 UTF-8 `String`。
- 运行宿主机命令并捕获输出。
- 当命中 `sudo` 路径时，把调用方显式提供的环境变量改写成 `env -- KEY=VALUE ...` 形式并放到提权后的目标命令边界内，避免只把变量注入到 `sudo` 自身进程环境，或把语义外包给宿主 `sudoers` 配置。
- `sudo` 可用性判定和 `sudo` 可执行路径选择遵循同一份有效 `PATH`（优先采用调用方在请求里显式覆盖的 `PATH`）。
- 对需要走 `sudo` 的 bare command，如果目标命令在有效 `PATH` 中不存在，会在真正调用 `sudo` 之前返回 `CommandNotFound`。
- 对 `/usr/bin/apt-get` 这类显式系统路径，仍保留 `IfNonRootSystemCommand` 语义；相对路径或工作目录下的同名命令不会被误判成系统命令。
- 运行 host recipe，并把非零退出统一建模成结构化错误。
- `HostRecipeError::Display` 只输出退出状态和捕获字节数，不把完整 stdout/stderr 直接拼进错误字符串；需要原始输出的调用方仍可从结构化 `Output` 读取。
- 为常见系统包命令提供默认 `sudo` 模式选择。
- Unix 下对 bare system command 做 `sudo -n` 试探。
- 配置子进程以支持进程树清理；如果子进程没有被放进独立进程组，cleanup capture 会 fail-closed。
- 捕获进程树清理标识并执行 best-effort 终止。
- Windows 下先等待 `taskkill /T /F` 的真实退出结果；只有它失败时才回退到 descendant sweep。
- Unix 上一旦无法重新验证原始 leader 身份，默认停止继续对该 process-group 做 `killpg`；但 Linux 会在 `/proc` 中继续回扫同 session 的残留成员，因此即使 leader 在 cleanup capture 之后退出，仍能清理原 process-group 里的 orphan descendants，同时对“capture 时已丢失 leader 身份且 cleanup 时 leader PID 已被复用”的情况继续 fail closed。

## 不负责什么

- 命令 allowlist。
- 超时、取消或重试策略。
- 环境变量过滤。
- stdout/stderr 的产品级脱敏、裁剪或持久化策略；这里仅避免在默认 `Display` 中直接倾倒完整捕获内容。
- sandbox / isolation 选择。
- 产品级错误码映射。

## 调用方边界

- 上层调用方负责决定何时执行命令以及失败后如何处理。
- 如果调用方需要跨平台可移植的 env 名约束或更高层过滤规则，应在自己边界处理；这里保留宿主原生字符串。
- 这里不拥有产品级安全策略。
