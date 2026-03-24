# FeishuWebhookSink

`FeishuWebhookSink` 通过飞书群机器人 webhook 发送消息（可选签名），支持：

- 默认 `text` 消息
- 对 `Event.body` 中的 Markdown 自动转 `post` 富文本
- Markdown 图片（`![alt](...)`）可选自动上传为飞书图片并内嵌显示

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx");
let sink = FeishuWebhookSink::new(cfg)?;
# Ok(())
# }
```

默认发送前会做 DNS 公网 IP 校验；如果你希望在 **构造阶段** 也校验一次（可能导致无网络时构造失败），可以用：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx");
let sink = FeishuWebhookSink::new_strict(cfg)?;
# Ok(())
# }
```

## 签名（可选）

如果群机器人开启了 “签名校验”，可以用：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx");
let sink = FeishuWebhookSink::new_with_secret(cfg, "your_secret")?;
# Ok(())
# }
```

每次发送会自动填充 `timestamp` / `sign` 字段，并且不会在 `Debug`/错误信息中泄露 secret 或完整 webhook URL。

如果你需要同时启用签名 + DNS 公网 IP 校验，并且希望在 **构造阶段** 也校验一次，可以用：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx");
let sink = FeishuWebhookSink::new_with_secret_strict(cfg, "your_secret")?;
# Ok(())
# }
```

## 超时

`FeishuWebhookConfig` 自带一个 HTTP timeout（默认 `2s`）。此外，`Hub` 也会对每个 sink 做兜底超时：

- 建议：`HubConfig.per_sink_timeout` ≥ `FeishuWebhookConfig.timeout`
- 如果你把 `Hub` 的超时设得更小，那么即使 HTTP 还没超时，也会被 `Hub` 先中断（drop future）

## 消息长度

`FeishuWebhookConfig.max_chars` 用于限制文本内容长度（默认 `4000`，超出会截断并追加 `...`）：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx")
    .with_max_chars(1000);
let sink = FeishuWebhookSink::new(cfg)?;
# Ok(())
# }
```

## DNS 公网 IP 校验开关

为降低 SSRF/DNS 污染风险，默认发送前会做一次 DNS 公网 IP 校验；如确有需要可关闭：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx")
    .with_public_ip_check(false);
let sink = FeishuWebhookSink::new(cfg)?;
# Ok(())
# }
```

## 安全约束（重要）

为降低 SSRF/凭据泄露风险，本库会对 webhook URL 做限制：

- 必须是 `https`
- 不允许携带 username/password
- host 仅允许：
  - `open.feishu.cn`
  - `open.larksuite.com`
- path 必须以 `/open-apis/bot/v2/hook/` 开头
- 不允许 `localhost` 或 IP
- 如显式指定端口，仅允许 `443`
- 禁用重定向（redirect）
- `Debug` 输出默认脱敏（不会泄露完整 webhook URL）

## 输出格式

默认 text 模式下，文本内容由以下部分组成（按顺序）：

1) `title`
2) `body`（如果存在且非空）
3) 每个 tag：`key=value`（逐行）

当 `body` 是 Markdown 且启用富文本（默认启用）时：

- 使用飞书 `post` 结构发送
- 链接会映射为可点击富文本链接
- 图片：
  - 未配置应用凭据时：降级为可读文本 + 原链接
  - 配置了应用凭据时：自动上传并以内嵌图片显示

## Markdown 图片上传（可选）

如果你希望 Markdown 图片真正显示为“图片”而不是链接，需要提供应用凭据：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx")
    .with_app_credentials("cli_xxx", "app_secret_xxx")
    .with_image_upload_max_bytes(10 * 1024 * 1024);
let sink = FeishuWebhookSink::new(cfg)?;
# Ok(())
# }
```

说明：

- 图片 URL 仅支持 `https`
- 本地文件路径默认禁用；如确实需要，可显式调用 `.with_local_image_files(true)` 后再读取并上传
- 上传失败时不会中断整条消息，自动回退为文本链接表示

## 错误信息（刻意保持“低敏感”）

为避免泄露敏感信息：

- 请求失败时的错误会被简化为类别（例如 `timeout/connect/request/...`）
- 非 2xx 的响应不会包含 response body（避免 body 中包含内部信息）
