# SlackWebhookSink

`SlackWebhookSink` 通过 Slack Incoming Webhook 发送 **text** 消息。

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use std::time::Duration;

use notify_kit::{SlackWebhookConfig, SlackWebhookSink};

let cfg = SlackWebhookConfig::new("https://hooks.slack.com/services/xxx")
    .with_timeout(Duration::from_secs(2))
    .with_max_chars(4000)
    // 可选：关闭 DNS 公网 IP 校验（默认开启；无网络/DNS 不可用时可能导致发送失败）
    .with_public_ip_check(false);

let sink = SlackWebhookSink::new(cfg)?;
# Ok(())
# }
```

## 安全约束（重要）

为降低 SSRF/凭据泄露风险，本库会对 webhook URL 做限制：

- 必须是 `https`
- 不允许携带 username/password
- host 仅允许：`hooks.slack.com`
- path 必须以 `/services/` 开头
- 不允许 `localhost` 或 IP
- 如显式指定端口，仅允许 `443`
- 禁用重定向（redirect）
- `Debug` 输出默认脱敏（不会泄露完整 webhook URL）

## 输出格式

文本内容由以下部分组成（按顺序）：

1) `title`
2) `body`（如果存在且非空）
3) 每个 tag：`key=value`（逐行）

## 长度限制

`SlackWebhookConfig.max_chars` 用于限制最终消息长度（超出会截断并追加 `...`）。
