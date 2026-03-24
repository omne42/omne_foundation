# API

本章按类型介绍 `notify-kit` 的核心 API。建议同时参考 Rustdoc（`cargo doc`）。

## 主要类型

- `Hub` / `HubConfig`：通知中心与配置
- `Event` / `Severity`：事件数据结构与严重级别
- `Sink`：通知渠道抽象
- `SoundSink` / `SoundConfig`：本地提示音/终端 bell
- `FeishuWebhookSink` / `FeishuWebhookConfig`：飞书 webhook（text / post 富文本，支持 Markdown 图片上传）
