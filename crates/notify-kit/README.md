# notify-kit

源码入口：[`src/lib.rs`](./src/lib.rs)  
详细文档：[`docs/README.md`](./docs/README.md)

## 领域

`notify-kit` 负责通知事件分发。

它把统一事件模型路由到多个 sink，并在不阻塞主流程的前提下处理并发发送和超时兜底。

推荐分层入口：

- `notify_kit::core`：`Event` / `Hub` / `Sink` / `Error` 这类 provider-agnostic foundation surface
- `notify_kit::providers::*`：内置 provider/integration sink
- `notify_kit::env`：可选的标准 env bootstrap helper，不属于核心通知抽象

## 边界

负责：

- 统一事件模型
- `Hub` 调度
- `Sink` 抽象
- 各通知渠道投递
- 并发发送和 per-sink timeout
- 错误聚合

不负责：

- 业务事件生成
- 可靠消息队列
- 复杂重试与持久化投递策略

## 范围

覆盖：

- `Event`
- `Severity`
- `Hub`
- `HubLimits`
- 一组内置 webhook / bot sink
- 标准环境变量接线 helper

不覆盖：

- 强投递保证
- 消息幂等与审计
- 高吞吐有序通知系统

## 结构设计

- `src/event.rs`
  - 统一事件模型
- `src/hub.rs`
  - hub 配置、限制、并发发送与错误聚合
- `src/core.rs`
  - provider-agnostic foundation re-export
- `src/providers.rs`
  - 内置 sink 的 feature-gated 命名空间
- `src/sinks/mod.rs`
  - sink trait 与各实现导出
- `src/sinks/http/`
  - webhook 类 sink 的共享 HTTP 逻辑
- `src/env.rs`
  - convenience helper，不是核心协议边界
- `bots/`
  - 上层集成示例，不是核心 Rust API

## 与其他 crate 的关系

- 依赖 `log-kit` 统一关键 warning 的稳定日志 code 与字段
- 详细用法和 sink 专题文档放在 crate 自己的 `docs/`

一个轻量的通知 Hub（Rust），用于把任意事件推送到多个通知渠道（sinks）。

当前实现：

- `sound`：终端 bell（默认）或自定义播放命令
- `feishu`：飞书群机器人 webhook（text 消息，可选签名）
- `github`：GitHub Issues/PR 评论（text）
- `slack`：Slack Incoming Webhook（text 消息）
- `discord`：Discord webhook（text 消息）
- `telegram`：Telegram Bot API（sendMessage）
- `serverchan`：Server酱（ServerChan）推送（text）
- `pushplus`：PushPlus 推送（text）
- `bark`：Bark 推送（text）
- `webhook`：通用 JSON webhook（`{text: ...}` 或自定义字段）
- `dingtalk`：钉钉群机器人 webhook（text 消息，可选签名）
- `wecom`：企业微信群机器人 webhook（text 消息）

设计目标：

- 可扩展：后续追加 email/discord/slack/tgbot/桌宠…只需要新增 sink
- 不阻塞：通知发送失败/超时不会卡住主流程（每个 sink 有超时）

## 安装

当前 `notify-kit` 仅按 workspace / Git 源方式使用：

```toml
[dependencies]
notify-kit = { path = "crates/notify-kit" }
```

如果你在其他仓库中引用，请改成对应的 Git 源或本地路径。

内置 sinks 现在按 feature 裁剪：

```toml
[dependencies]
notify-kit = { path = "crates/notify-kit", default-features = false, features = ["sink-sound"] }
```

- `default` 仍然保留现有默认体验：启用全部内置 sinks 与 `env-standard`
- `default-features = false` 时，核心 `Event` / `Hub` / `Sink` 仍可用，只是不再自动带入内置 transport sinks
- 标准 env helper `notify_kit::env` 现在需要 `env-standard` feature；它会自动带上自己依赖的 `sound` / `generic-webhook` / `slack` / `feishu` sinks
- 额外的 `sound-command` 仍然是对 `SoundSink` 的增量 opt-in，需要与 `sink-sound` 一起使用（或沿用默认 features）
- 新代码建议优先从 `notify_kit::core` 和 `notify_kit::providers::*` 引用类型；crate root 仍保留兼容 re-export，但不再是推荐的边界表达方式

## 文档

- mdBook：`docs/README.md`（目录：`docs/SUMMARY.md`）
- 本地预览（含搜索）：`./scripts/docs.sh serve`（需要先 `cargo install mdbook --locked`）
- Rustdoc：`cargo doc -p notify-kit --open`
- LLM 友好入口：`llms.txt`（由 `./scripts/build-llms-txt.sh` 生成）

## Bots（上层集成示例）

本仓库的核心是 Rust 通知库（`Hub` + `sinks`）。另外也提供少量“上层 bot/集成示例”：

- `bots/`（见 `bots/README.md`）

## 用法

`Hub::notify_best_effort` 是显式的 best-effort fire-and-forget：在 **Tokio runtime** 中 spawn 后台任务并立即返回。

- 如果当前没有 Tokio runtime：`notify_best_effort` 会丢弃通知并 `tracing::warn!`；可用 `Hub::try_notify` 检测。
- 如果需要可观测结果：用 `Hub::send(event).await`（会等待所有 sinks 完成/超时）。
- 旧的 `Hub::notify` 仍保留为兼容别名，但已弃用；新代码应改用 `Hub::notify_best_effort` 或 `Hub::try_notify`。
- 注意：`HubConfig.per_sink_timeout` 是 Hub 对每个 sink 的兜底超时；如果你把某个 sink 的 `timeout` 调大，也需要把 `per_sink_timeout` 调到 >= 该值，否则 Hub 可能会先超时。
- 运行时限制（例如 `max_inflight_events`、`max_sink_sends_in_parallel`）放在 `HubLimits`，避免把执行期背压策略混进 `HubConfig` 的语义配置里。

如果你需要显式控制这些限制，可用 `Hub::new_with_limits(...)` 搭配 `HubLimits::default().with_max_inflight_events(...).with_max_sink_sends_in_parallel(...)`。

最小示例（需要在 Tokio runtime 中调用）：

```rust
use std::sync::Arc;

use notify_kit::core::{Event, Hub, HubConfig, Severity};
use notify_kit::providers::sound::{SoundConfig, SoundSink};

let hub = Hub::new(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
);

hub.notify_best_effort(Event::new("turn_completed", Severity::Success, "done"));
```

## 安全提示

- `SoundConfig.command_argv` 会执行外部命令（需要启用 `notify-kit/sound-command`）；应视为 **受信任/本机配置**。
- `FeishuWebhookSink` 会校验 webhook URL：仅允许 `https` + `open.feishu.cn` / `open.larksuite.com`，且不会在 `Debug`/错误信息中输出完整 URL。
- `FeishuWebhookSink` 默认不会因为 Markdown 正文里出现远程图片 URL 就主动发起下载；远程图片上传必须显式 `with_remote_image_urls(true)`，本地图片也必须显式 `with_local_image_files(true)`。绝对路径可以直接读取；相对路径必须额外显式配置 `with_local_image_base_dir(...)`，库不会再偷偷把进程 `cwd` 当作图片解析输入。本地图片读取会复用 workspace 的 no-follow 文件系统原语；如果宿主平台拿不到这条安全边界，会直接 fail closed。

## 配置（环境变量）

本库不规定统一的环境变量协议；配置应由上层应用负责（比如 integration 层解析 env，然后构造 sinks + Hub）。

如果你需要库自带的快捷接线方式，推荐使用：

- `notify_kit::env::build_hub_from_standard_env(...)`
- `notify_kit::env::StandardEnvHubOptions`

它们只是 convenience helper，适合快速接线或共享一套简单约定；不是强制协议，也不是核心架构边界。
核心 `Hub` / `Sink` API 继续留在 crate root；env helper 刻意只挂在 `notify_kit::env::...`，避免把 `NOTIFY_*` 约定伪装成通知 foundation 的默认接线协议。
新代码更推荐从 `notify_kit::core::{...}` 读取核心抽象、从 `notify_kit::providers::<provider>::{...}` 读取具体 sink。
这套 helper 自带的中性约定是 `NOTIFY_*`，例如 `NOTIFY_SOUND`、`NOTIFY_WEBHOOK_URL`、`NOTIFY_TIMEOUT_MS`、`NOTIFY_EVENTS`。
公开入口就是 `notify_kit::env::...`；不要在 crate root 上再叠一层快捷别名。

## 标准 helper 示例

上层应用也可以直接沿用这套中性约定；如果需要产品专属前缀或额外字段，建议在应用侧单独封装 integration crate。示例：

```bash
export NOTIFY_SOUND=1
# export NOTIFY_FEISHU_WEBHOOK_URL="..."
# export NOTIFY_EVENTS="turn_completed,approval_requested,message_received"

cargo run -p your-app
```

## 开发

离线检查：

```bash
CARGO_NET_OFFLINE=true ./scripts/gate.sh
```
