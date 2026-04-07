# API

本章按类型介绍 `notify-kit` 的核心 API。建议同时参考 Rustdoc（`cargo doc`）。

推荐入口：

- `notify_kit::core::{...}`：核心 foundation 抽象
- `notify_kit::providers::<provider>::{...}`：具体 sink/config
- `notify_kit::env::{...}`：可选标准 env helper

## 主要类型

- `Hub` / `HubConfig`：通知中心与配置
- `Event` / `Severity`：事件数据结构与严重级别
- `Sink`：通知渠道抽象
- `providers::sound::SoundSink` / `SoundConfig`：本地提示音/终端 bell
- `providers::feishu::FeishuWebhookSink` / `FeishuWebhookConfig`：飞书 webhook（text / post 富文本，支持显式 opt-in 的 Markdown 图片上传）
