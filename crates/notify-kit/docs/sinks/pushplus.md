# PushPlusSink

`PushPlusSink` 通过 PushPlus API 发送推送（纯文本）。

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{PushPlusConfig, PushPlusSink};

let cfg = PushPlusConfig::new("your_token");
let sink = PushPlusSink::new(cfg)?;
# Ok(())
# }
```

可选：设置 channel / topic / template：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{PushPlusConfig, PushPlusSink};

let cfg = PushPlusConfig::new("your_token")
    .with_channel("wechat")
    .with_template("txt")
    .with_topic("my_topic");
let sink = PushPlusSink::new(cfg)?;
# Ok(())
# }
```

## 超时

`PushPlusConfig` 自带 HTTP timeout（默认 `2s`）。此外，`Hub` 也会对每个 sink 做兜底超时：

- 建议：`HubConfig.per_sink_timeout` ≥ `PushPlusConfig.timeout`

## 安全提示

- `token` 属于敏感信息：不要写入日志/错误信息/Debug 输出。
- 默认会做 DNS 公网 IP 校验（可通过 `with_public_ip_check(false)` 关闭）。
