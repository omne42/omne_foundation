# 内置 Sinks

本章介绍 `notify-kit` 内置的 sinks，并给出常见配置示例。

内置 sinks：

- 速览（字段/能力可能随版本演进；以各 sink 章节为准）：

| sink | 发送对象 | 认证/必填 | 备注 |
| --- | --- | --- | --- |
| `sound` | 本机终端 | 无 / `command_argv`（可选） | 终端 bell 可触发 Visual Bell / Dock/任务栏提示（取决于终端设置） |
| `feishu` | 飞书群机器人 | `webhook_url`（可选签名 secret） | host allow-list + 可选公网 IP 校验 |
| `dingtalk` | 钉钉群机器人 | `webhook_url`（可选签名 secret） | host allow-list + 可选公网 IP 校验 |
| `wecom` | 企业微信群机器人 | `webhook_url` | host allow-list + 可选公网 IP 校验 |
| `slack` | Slack Incoming Webhook | `webhook_url` | host allow-list + 可选公网 IP 校验 |
| `discord` | Discord Webhook | `webhook_url` | host allow-list + 可选公网 IP 校验 |
| `telegram` | Telegram Bot API | `bot_token` + `chat_id` | 走官方 API 域名 |
| `github` | GitHub 评论 | `token` + `repo/issue` | 走 GitHub API |
| `serverchan` | ServerChan | `send_key` | 走官方 API |
| `pushplus` | PushPlus | `token` | 走官方 API |
| `bark` | Bark | `device_key` | 走官方 API |
| `webhook` | 通用 webhook | `url`（建议 strict） | 非 strict 模式请只用于可信配置 |

- `sound`：终端 bell / 外部命令
- `feishu`：飞书 webhook
- `github`：GitHub 评论（Issues/PR）
- `slack`：Slack Incoming Webhook
- `discord`：Discord webhook
- `telegram`：Telegram Bot API
- `serverchan`：Server酱（ServerChan）
- `pushplus`：PushPlus
- `bark`：Bark
- `webhook`：通用 JSON webhook
- `dingtalk`：钉钉 webhook
- `wecom`：企业微信 webhook

如果你需要额外渠道（Email/Push/自建系统…），请看 [自定义 Sink](custom.md)。
