# FeishuWebhookSink

`FeishuWebhookSink` 通过飞书群机器人 webhook 发送消息（可选签名），支持：

- 默认 `text` 消息
- 对 `Event::body()` 中的 Markdown 自动转 `post` 富文本
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

本地图片额外约束：

- 只有显式 `with_local_image_files(true)` 后才允许读取本地文件
- 必须配置绝对路径 `local_image_root(s)` allow-list
- 相对路径不会再隐式相对进程 `cwd` 解析；如果要允许相对路径，必须显式设置 `with_local_image_base_dir(...)`

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
  - 配置了应用凭据，但没有显式开启图片来源时：仍然降级为可读文本 + 原链接
  - 只有在显式开启远程 URL 或本地文件来源后，才会尝试上传并以内嵌图片显示

## Markdown 图片上传（可选）

如果你希望 Markdown 图片真正显示为“图片”而不是链接，建议把这组更宽的媒体能力显式放进 `FeishuWebhookMediaConfig`：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookMediaConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx")
    .with_media_config(
        FeishuWebhookMediaConfig::new()
            .with_app_credentials("cli_xxx", "app_secret_xxx")
            .with_image_upload_max_bytes(10 * 1024 * 1024),
    );
let sink = FeishuWebhookSink::new(cfg)?;
# Ok(())
# }
```

说明：

- 远程图片 URL 默认禁用；如确实需要，可显式调用 `.with_remote_image_urls(true)` 后再允许下载并上传
- 远程图片下载始终强制做 DNS 公网 IP 校验；即使 webhook 主请求显式关闭了 `with_public_ip_check(false)`，图片下载也不会因此放宽到内网 / special-use 目标
- 图片 URL 仅支持 `https`
- 本地文件路径默认禁用；如确实需要，需要同时显式配置：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{FeishuWebhookConfig, FeishuWebhookMediaConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx")
    .with_media_config(
        FeishuWebhookMediaConfig::new()
            .with_app_credentials("cli_xxx", "app_secret_xxx")
            .with_local_image_files(true)
            .with_local_image_root("/abs/path/to/exported-images"),
    );
let sink = FeishuWebhookSink::new(cfg)?;
# Ok(())
# }
```

- 只会读取显式 `local_image_root(s)` 之下的文件；超出 root、`..` 逃逸、symlink 组件和其他特殊路径都会 fail closed
- 非 Unix 平台如果无法提供安全的 no-follow 打开语义，会直接拒绝本地图片读取，而不是退化成跟随 symlink/reparse point
- 上传失败时不会中断整条消息，自动回退为文本链接表示

## 错误信息（刻意保持“低敏感”）

为避免泄露敏感信息：

- 请求失败时的错误会被简化为类别（例如 `timeout/connect/request/...`）
- 非 2xx 的响应不会包含 response body（避免 body 中包含内部信息）
