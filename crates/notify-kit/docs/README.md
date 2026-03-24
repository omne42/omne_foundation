# notify-kit

一个轻量的通知 Hub（Rust），用于把任意事件推送到多个通知渠道（sinks）。

> 这是 `mdBook` 文档（入口：`docs/README.md`，目录：`docs/SUMMARY.md`）。

## 你将获得什么

- **统一出口**：用 `Hub` 把事件广播到多个 sink。
- **不阻塞主流程**：`Hub::notify` fire-and-forget；每个 sink 都有超时上限。
- **可扩展**：通过实现 `Sink` trait 接入任意渠道（Discord/Slack/Email/桌面通知…）。
- **安全意识**：对 webhook URL 进行限制；Debug 输出默认脱敏。

## 当前内置 sinks

内置 sinks：

- `sound`：终端 bell（默认）或执行外部播放命令
- `feishu`：飞书群机器人 webhook（text，可选签名）
- `github`：GitHub Issues/PR 评论（text）
- `slack`：Slack Incoming Webhook（text）
- `discord`：Discord webhook（text）
- `telegram`：Telegram Bot API（sendMessage）
- `serverchan`：Server酱（ServerChan）
- `pushplus`：PushPlus
- `bark`：Bark
- `webhook`：通用 JSON webhook
- `dingtalk`：钉钉群机器人 webhook（text，可选签名）
- `wecom`：企业微信群机器人 webhook（text）

## 适用场景

- CLI 工具：任务完成/失败时提示音 + 群通知
- 服务端：关键流程失败时告警到 IM（同时保留本地提示作为 fallback）
- agent / automation：把内部事件（turn/message/approval）路由到不同通知渠道

## 非适用场景（建议用更重的方案）

- 需要强投递保证、持久化队列、重试/退避策略、去重与合并等“可靠消息系统”
- 需要高吞吐（每秒大量事件）并且强一致的通知顺序控制

## 兼容性

- Rust edition：2024
- MSRV：Rust `1.85`

## 快速导航

- 入门： [快速开始](getting-started.md)
- 示例： [Examples / Recipes](examples.md)
- 概念： [核心概念](concepts.md)
- API： [Hub](api/hub.md)、[Event](api/event.md)、[Sink](api/sink.md)
- 内置 sinks： [内置 Sinks](sinks/README.md)
- Bots： [Bots（交互式集成示例）](bots.md)
- 集成： [集成与配置](integration.md)
- 安全： [安全](security.md)
- LLM 入口： [llms.txt](llms.md)

下一步：从 [快速开始](getting-started.md) 开始。
