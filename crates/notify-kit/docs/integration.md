# 集成与配置

本库**不规定**统一的环境变量协议；配置应由上层应用负责（例如解析 env，然后构造 sinks + Hub）。

如果你只想快速接线，库里也提供了：

- `notify_kit::env::build_hub_from_standard_env(...)`
- `notify_kit::env::StandardEnvHubOptions`

它们是 convenience helper，不是强制协议，也不改变推荐分层。

补充说明：

- root-level 的兼容 re-export 仅用于平滑迁移，并已标记为 deprecated。
- 新代码与文档示例应优先使用 `notify_kit::env::...` 路径。

## 一个推荐的配置层结构

```text
your-app
  ├─ config (env/cli/file)
  ├─ notify (integration layer)
  └─ business logic
```

在 integration layer 中：

1) 解析配置（例如 `NOTIFY_SOUND=1`、`NOTIFY_FEISHU_WEBHOOK_URL=...`）
2) 构造 sinks（`SoundSink`、`FeishuWebhookSink`、自定义 sinks）
3) 构造 `Hub` 并注入到业务逻辑

补充建议：

- `HubConfig` 放过滤、超时这类语义配置
- `HubLimits` 放 inflight 上限、sink fan-out 并行度这类执行期限制

## 一个参考的 env/CLI 协议（示例）

下面是一个“够用且可维护”的例子，你可以按需裁剪：

- `MYAPP_NOTIFY_SOUND=1`
- `MYAPP_NOTIFY_FEISHU_WEBHOOK_URL=...`
- `MYAPP_NOTIFY_EVENTS=turn_completed,approval_requested,message_received`
- `MYAPP_NOTIFY_TIMEOUT_MS=2000`

对应到 integration 层的伪代码：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use notify_kit::{
    FeishuWebhookConfig, FeishuWebhookSink, Hub, HubConfig, HubLimits, Sink, SoundConfig,
    SoundSink,
};

fn build_hub_from_env() -> notify_kit::Result<Hub> {
    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();

    if std::env::var("MYAPP_NOTIFY_SOUND").ok().as_deref() == Some("1") {
        sinks.push(Arc::new(SoundSink::new(SoundConfig { command_argv: None })));
    }

    if let Ok(url) = std::env::var("MYAPP_NOTIFY_FEISHU_WEBHOOK_URL") {
        sinks.push(Arc::new(FeishuWebhookSink::new(FeishuWebhookConfig::new(url))?));
    }

    let enabled_kinds = std::env::var("MYAPP_NOTIFY_EVENTS")
        .ok()
        .map(|s| s.split(',').filter(|x| !x.trim().is_empty()).map(|x| x.trim().to_string()).collect::<BTreeSet<_>>());

    let per_sink_timeout = std::env::var("MYAPP_NOTIFY_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(5));

    Ok(Hub::new_with_limits(
        HubConfig {
            enabled_kinds,
            per_sink_timeout,
        },
        sinks,
        HubLimits::default(),
    ))
}
```

如果你采用库自带的 env helper，建议通过 `notify_kit::env::build_hub_from_standard_env(...)` 访问，并把它当成 bootstrap helper：能减少样板代码，但不妨碍你在自己的 integration layer 继续包装、替换或扩展。

## 与 omne-agent 的集成（示例）

`omne-agent` 仓库（目录名为 `omne-agent/`）内的 `omne-agent-app-server` notify integration 负责解析 `OMNE_AGENT_NOTIFY_*` 并构造 Hub。

```bash
cd ../omne-agent

export OMNE_AGENT_NOTIFY_SOUND=1
# export OMNE_AGENT_NOTIFY_FEISHU_WEBHOOK_URL="..."
# export OMNE_AGENT_NOTIFY_EVENTS="turn_completed,approval_requested,message_received"

cargo run -p omne-agent-app-server --features notify
```
