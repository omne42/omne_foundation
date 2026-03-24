# GenericWebhookSink

`GenericWebhookSink` 会向指定 URL POST 一个 JSON payload（默认 `{ "text": "..." }`）。

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{GenericWebhookConfig, GenericWebhookSink};

let cfg = GenericWebhookConfig::new("https://example.com/webhook");
let sink = GenericWebhookSink::new(cfg)?;
# Ok(())
# }
```

可选：修改字段名、限制 URL path 前缀、限制允许的 host：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{GenericWebhookConfig, GenericWebhookSink};

let cfg = GenericWebhookConfig::new("https://example.com/hooks/notify")
    .with_payload_field("content")
    .with_path_prefix("/hooks/")
    .with_allowed_hosts(vec!["example.com".to_string()]);
let sink = GenericWebhookSink::new(cfg)?;
# Ok(())
# }
```

## 严格模式（推荐）

如果 webhook URL 可能来自**不可信输入/远程配置**，建议使用严格模式：强制配置 `allowed_hosts` + `path_prefix`，并且不能关闭 DNS 公网 IP 校验：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{GenericWebhookConfig, GenericWebhookSink};

let cfg = GenericWebhookConfig::new_strict(
    "https://example.com/hooks/notify",
    "/hooks/",
    vec!["example.com".to_string()],
);
let sink = GenericWebhookSink::new_strict(cfg)?;
# Ok(())
# }
```

## 安全提示

- 默认会做 DNS 公网 IP 校验（可通过 `with_public_ip_check(false)` 关闭；出于安全考虑，关闭时必须同时配置 `allowed_hosts`）。
- 如果你使用 `allowed_hosts`，建议把它视为安全边界（不要从不可信输入构造）；不确定时用上面的严格模式。
