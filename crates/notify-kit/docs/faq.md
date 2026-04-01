# FAQ / 排错

## 为什么 `notify()` 没有任何效果？

最常见原因有两个：

- 当前线程不在 Tokio runtime 中，此时 `Hub::notify` 会返回 `TryNotifyError::NoTokioRuntime`
- Hub inflight 容量已满，此时 `Hub::notify` 会返回 `TryNotifyError::Overloaded`

如果你明确接受“失败时记录 warning 并丢弃”，请使用 `Hub::notify_lossy(...)`。

## 为什么 `try_notify()` 返回了 `NoTokioRuntime`？

`try_notify()` 和 `notify()` 都会调用 `tokio::runtime::Handle::try_current()`：

- 若当前线程不在 Tokio runtime 内，会返回 `NoTokioRuntime`
- 这是有意为之：通知是附加能力，不应让调用方被迫引入 runtime 或 panic

## `send()` 返回了 timeout？

`HubConfig.per_sink_timeout` 是每个 sink 的兜底超时：

- 调大 `per_sink_timeout`
- 或优化/拆分你的 sink（避免单次发送太慢）

## `send()` 返回了聚合错误，我该怎么处理？

聚合错误代表“至少一个 sink 失败了”。常见处理方式：

- 对关键通知：把 `hub.send(event).await?` 当作必须成功的步骤（失败则上报/重试）
- 对非关键通知：记录 warning 并继续主流程（例如 `tracing::warn!(...)`）

## `Event` 里的 `StructuredText` 为什么没有自动本地化？

`notify-kit` 允许 `Event` 保留 `StructuredText`，但内置 sinks 不负责 locale-aware 渲染：

- freeform 文本会原样输出
- catalog-backed `StructuredText` 只会降级成稳定字符串

如果你需要最终用户可见的本地化文案，请先在上层用自己的 catalog/runtime 渲染成普通文本，再交给 `Event::new(...)` / `with_body(...)` / `with_tag(...)`。

## Feishu webhook 报 host is not allowed？

本库只允许 `open.feishu.cn` / `open.larksuite.com`：

- 确认你使用的是群机器人 webhook 的标准域名
- 不支持自定义代理域名（避免 SSRF 风险）

## 如何让 TUI 在回复完成后让终端“闪一下”（macOS/Windows）？

思路：在“回复完成”事件上触发 `SoundSink` 的终端 bell（`\u{0007}`）。

- `notify-kit` 侧：使用 `SoundSink`（默认就是 bell）。
- 终端/系统侧：需要你在终端设置里启用 Visual Bell / Dock/任务栏提示（不同终端选项不同，见 [SoundSink](sinks/sound.md)）。

本仓库不包含具体 TUI 应用；你需要在你的 TUI 项目里在“reply completed / turn completed”处调用 `hub.notify(...)`，并决定失败时是重试、降级还是显式改用 `hub.notify_lossy(...)`。
