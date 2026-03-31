# 快速开始

本章给出最小可运行示例，并解释 `notify` 与 `send` 的差异。

## 安装

如果你通过 crates.io 使用：

```toml
[dependencies]
notify-kit = "0.1"
```

如果你通过 Git / monorepo 引用：

```toml
[dependencies]
notify-kit = { path = "/path/to/omne_foundation/crates/notify-kit" }
```

> 以上版本与路径仅为示例；请按你的项目实际情况调整。

## 一个可运行的 `main.rs` 示例

`Hub::notify` 需要在 **Tokio runtime** 中调用；如果当前没有 Tokio runtime 或 Hub 已过载，它会返回显式错误，而不是静默丢弃。

```rust,no_run,edition2024
# extern crate notify_kit;
# extern crate tokio;
# extern crate tracing;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use notify_kit::{
    Event, Hub, HubConfig, HubLimits, Severity, Sink, SoundConfig, SoundSink, TryNotifyError,
};

fn main() -> notify_kit::Result<()> {
    // 组合多个 sinks（示例只启用 sound）
    let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))];

    // 可选：只允许一部分 kind
    let enabled_kinds: Option<BTreeSet<String>> =
        Some(BTreeSet::from(["turn_completed".to_string(), "approval_requested".to_string()]));

    let hub = Hub::new(
        HubConfig {
            enabled_kinds,
            per_sink_timeout: Duration::from_secs(5),
        },
        sinks,
    );

    // `notify-kit` 需要在 Tokio runtime 中运行；这里用一个最小 runtime 来演示。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        // fire-and-forget（只负责入队，不等待 sink 完成）
        hub.notify(Event::new("turn_completed", Severity::Success, "done"))?;

        // 可观测结果（等待所有 sinks）
        hub.send(Event::new("turn_completed", Severity::Success, "done (awaited)"))
            .await?;

        // `try_notify` 仍保留为显式命名别名；当你更喜欢 "try_*" 语义时可以继续用它。
        match hub.try_notify(Event::new("turn_completed", Severity::Success, "done (try_notify)")) {
            Ok(()) => {}
            Err(TryNotifyError::NoTokioRuntime) => {
                // 这里不要 panic：通知通常只是附加能力。
                // 你可以选择：记录日志、降级为 stdout、暂存到队列里、或忽略。
                tracing::debug!("no tokio runtime; notification skipped");
            }
            Err(TryNotifyError::Overloaded) => {
                // 运行时限流生效：说明当前 Hub 已经处于忙碌状态。
                // 这里同样建议降级处理，而不是影响主流程。
                tracing::debug!("hub overloaded; notification skipped");
            }
        }

        Ok::<_, notify_kit::Error>(())
    })?;

    Ok(())
}
```

如果你需要调节运行时背压，而不是语义配置，可以改用 `HubLimits`：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::sync::Arc;

use notify_kit::{Hub, HubConfig, HubLimits, SoundConfig, SoundSink};

let hub = Hub::new_with_limits(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
    HubLimits::default()
        .with_max_inflight_events(64)
        .with_max_sink_sends_in_parallel(8),
);
```

## 我该用 `notify` 还是 `send`？

- `notify(event) -> Result<(), TryNotifyError>`: fire-and-forget，只保证事件成功入队
- `notify_lossy(event)`: 明确选择“尽力而为 + warning 日志 + 失败时丢弃”
- `try_notify(event) -> Result<(), TryNotifyError>`: `notify` 的显式别名
- `send(event).await -> notify_kit::Result<()>`: 等待所有 sinks 完成/超时，并聚合错误信息

## 常见模式

### 同时启用多个 sinks

```rust,no_run,edition2024
#
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use std::sync::Arc;

use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink, Sink, SoundConfig, SoundSink};

let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
// 本地提示音
sinks.push(Arc::new(SoundSink::new(SoundConfig { command_argv: None })));
// 飞书 webhook（注意：webhook URL 属于敏感信息，请用安全配置注入）
sinks.push(Arc::new(FeishuWebhookSink::new(FeishuWebhookConfig::new(
    "https://open.feishu.cn/open-apis/bot/v2/hook/xxx",
))?));
#
# Ok(())
# }
```

### 事件过滤（只发你关心的 kind）

```rust,no_run,edition2024
# extern crate notify_kit;
use std::collections::BTreeSet;
use std::time::Duration;

use notify_kit::HubConfig;

let enabled_kinds = BTreeSet::from(["turn_completed".to_string(), "message_received".to_string()]);
let cfg = HubConfig {
    enabled_kinds: Some(enabled_kinds),
    per_sink_timeout: Duration::from_secs(5),
};
```

这里的 `HubConfig` 只描述过滤与超时。像 inflight 上限、单事件 fan-out 并行度这类运行时限制，放到 `HubLimits` 中更合适。
