# Examples / Recipes

这里提供一些可复制的使用片段，用于把 `notify-kit` 快速接入到你的 CLI/服务端/CI/agent 系统中。

> 这些例子关注“调用方式与模式”，具体环境变量解析/配置管理建议放在你的 integration 层完成。

## 1) CLI：任务完成时响铃（并可在终端里配置 Dock/任务栏闪烁）

```rust,no_run,edition2024
# extern crate notify_kit;
# extern crate tokio;
# fn main() -> notify_kit::Result<()> {
use std::sync::Arc;

use notify_kit::{Event, Hub, HubConfig, Severity, SoundConfig, SoundSink};

let hub = Hub::new(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
);

let rt = tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()
    .expect("build tokio runtime");
rt.block_on(hub.send(Event::new("task_done", Severity::Success, "done")))?;
# Ok(())
# }
```

提示：如果你希望“macOS Dock/Windows 任务栏闪一下”，通常需要在终端设置里开启 Visual Bell / Bounce（见 [SoundSink](sinks/sound.md)）。

## 2) 服务端：关键错误同时发到多个渠道

```rust,no_run,edition2024
#
# extern crate notify_kit;
# extern crate tokio;
# fn main() -> notify_kit::Result<()> {
use std::sync::Arc;

use notify_kit::{Event, Hub, HubConfig, Severity, SoundConfig, SoundSink};
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let sink_sound = Arc::new(SoundSink::new(SoundConfig { command_argv: None }));
let sink_feishu = Arc::new(FeishuWebhookSink::new(FeishuWebhookConfig::new(
    "https://open.feishu.cn/open-apis/bot/v2/hook/...",
))?);

let hub = Hub::new(HubConfig::default(), vec![sink_sound, sink_feishu]);
let rt = tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()
    .expect("build tokio runtime");
rt.block_on(hub.send(
    Event::new("fatal", Severity::Error, "service crashed").with_body("trace id: ..."),
))?;
#
# Ok(())
# }
```

## 3) CI：失败时发到通用 webhook

```rust,no_run,edition2024
#
# extern crate notify_kit;
# extern crate tokio;
# fn main() -> notify_kit::Result<()> {
use std::sync::Arc;

use notify_kit::{Event, Hub, HubConfig, Severity};
use notify_kit::{GenericWebhookConfig, GenericWebhookSink};

let sink = Arc::new(GenericWebhookSink::new_strict(GenericWebhookConfig::new_strict(
    "https://example.com/webhook/notify",
    "/webhook/",
    vec!["example.com".into()],
))?);

let hub = Hub::new(HubConfig::default(), vec![sink]);
let rt = tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()
    .expect("build tokio runtime");
rt.block_on(hub.send(Event::new("ci_failed", Severity::Error, "build failed")))?;
#
# Ok(())
# }
```

## 4) Agent：只打开少数事件（kind allow-list）

```rust,no_run,edition2024
#
# extern crate notify_kit;
# extern crate tokio;
# fn main() -> notify_kit::Result<()> {
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use notify_kit::{Event, Hub, HubConfig, Severity, SoundConfig, SoundSink};

let enabled_kinds = BTreeSet::from(["turn_completed".to_string(), "approval_requested".to_string()]);
let hub = Hub::new(
    HubConfig {
        enabled_kinds: Some(enabled_kinds),
        per_sink_timeout: Duration::from_secs(5),
    },
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
);

hub.notify(Event::new("turn_completed", Severity::Success, "done"));
hub.notify(Event::new("debug_noise", Severity::Info, "ignored"));
#
# Ok(())
# }
```
