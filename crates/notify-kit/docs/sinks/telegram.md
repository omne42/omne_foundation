# TelegramBotSink

`TelegramBotSink` 通过 Telegram Bot API（`sendMessage`）发送文本消息。

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use std::time::Duration;

use notify_kit::{TelegramBotConfig, TelegramBotSink};

let cfg = TelegramBotConfig::new("bot_token", "chat_id")
    .with_timeout(Duration::from_secs(2))
    .with_max_chars(4096);

let sink = TelegramBotSink::new(cfg)?;
# Ok(())
# }
```

## 安全约束（重要）

- Bot token 属于敏感信息：`Debug`/错误信息不会输出 token。
- 发送 endpoint 固定为官方域名：`api.telegram.org`（不支持自定义 base_url，避免 SSRF）。

## 输出格式

文本内容由以下部分组成（按顺序）：

1) `title`
2) `body`（如果存在且非空）
3) 每个 tag：`key=value`（逐行）

## 长度限制

`TelegramBotConfig.max_chars` 用于限制最终消息长度（超出会截断并追加 `...`）。
