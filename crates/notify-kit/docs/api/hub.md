# Hub

`Hub` 是通知中心：把一个 `Event` 广播到多个 sinks。

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
use std::sync::Arc;
use notify_kit::{Hub, HubConfig, SoundConfig, SoundSink};

let hub = Hub::new(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
);
```

如果你需要对 `notify()` 的后台并发做背压（避免事件洪泛导致无界 spawn），可以用：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::sync::Arc;
use notify_kit::{Hub, HubConfig, SoundConfig, SoundSink};

let hub = Hub::new_with_inflight_limit(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
    32,
);
```

当 inflight 超过上限时，`notify()` 会丢弃该条通知并记录 warning；`send().await` 会等待额度释放。

如果你还需要限制“单个事件扇出时，最多同时发送多少个 sink”，可以显式传入 `HubLimits`：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::sync::Arc;
use notify_kit::{Hub, HubConfig, HubLimits, SoundConfig, SoundSink};

let hub = Hub::new_with_limits(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
    HubLimits::default()
        .with_max_inflight_events(32)
        .with_max_sink_sends_in_parallel(4),
);
```

## HubConfig

- `enabled_kinds: Option<BTreeSet<String>>`
  - `None`：不过滤
  - `Some(set)`：仅允许 set 内 kind
- `per_sink_timeout: Duration`
  - 默认 `5s`
  - 作为兜底，避免任何 sink 卡住调用方

`HubConfig` 只表达“Hub 的语义配置”，例如过滤和超时。

## HubLimits

- `max_inflight_events: usize`
  - 默认 `128`
  - 控制 Hub 内同时在途的事件数量（`notify()` 后台任务和 `send().await` 都受影响）
- `max_sink_sends_in_parallel: usize`
  - 默认 `16`
  - 控制单个事件 fan-out 时，最多并行发送多少个 sinks

`HubLimits` 表达的是“执行期限制 / 背压策略”，故意与 `HubConfig` 分开，避免配置职责混杂。

推荐通过 `HubLimits::default().with_...(...)` 逐项覆盖，而不是把它当成业务语义配置对象来扩展。

一个更完整的配置示例：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::collections::BTreeSet;
use std::time::Duration;

use notify_kit::HubConfig;

let enabled_kinds = BTreeSet::from(["turn_completed".to_string(), "approval_requested".to_string()]);
let cfg = HubConfig {
    enabled_kinds: Some(enabled_kinds),
    per_sink_timeout: Duration::from_secs(5),
};
```

## 发送接口

- `notify(event)`: fire-and-forget；无 runtime 时会丢弃并记录 warning
- `try_notify(event)`: 同上，但缺少 runtime 时返回 `TryNotifyError::NoTokioRuntime`
- `send(event).await`: 等待所有 sinks 完成/超时；失败时聚合错误并返回

## 行为细节

- **kind 被禁用时是 no-op**：即使没有 Tokio runtime 也不会报错（直接返回）。
- **并发发送**：`send().await` 会并发调用多个 sinks，并受 `HubLimits.max_sink_sends_in_parallel` 限制。
- **每个 sink 单独超时**：由 `per_sink_timeout` 控制；超时会被视为该 sink 失败。
- **错误聚合**：当一个或多个 sinks 失败时，会返回一个聚合错误，内容类似：

```text
one or more sinks failed:
- feishu: timeout after 5s
- sound: boom
```
