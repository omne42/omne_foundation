# Bots（交互式集成示例）

`notify-kit` 的核心是 Rust 通知库（`Hub` + `sinks`），用于把事件推送到外部渠道。

本仓库也提供少量“上层 bot / 集成示例”，用于把某个平台的消息桥接到 OpenCode 会话（session）：

- Slack：`bots/opencode-slack`（Socket Mode）
- Discord：`bots/opencode-discord`（Gateway）
- Telegram：`bots/opencode-telegram`（long polling）
- 飞书：`bots/opencode-feishu`（事件订阅 Webhook）
- 钉钉：`bots/opencode-dingtalk-stream`（Stream Mode）
- GitHub：`bots/opencode-github-action`（GitHub Actions：issue/pr 评论触发）
- 企微：`bots/opencode-wecom`（企业微信自建应用回调）

这些 bot 都遵循同一个最小模式：

1) 把对话上下文（thread/chat/sessionWebhook）映射为 OpenCode session
2) 把用户消息转发到 `session.prompt`
3) 把模型回复与 tool 完成状态回帖到同一对话中

## 快速开始

直接看各 bot 的 README：

- `bots/opencode-slack/README.md`
- `bots/opencode-discord/README.md`
- `bots/opencode-telegram/README.md`
- `bots/opencode-feishu/README.md`
- `bots/opencode-dingtalk-stream/README.md`
- `bots/opencode-github-action/README.md`
- `bots/opencode-wecom/README.md`

## 重要说明

- 这些示例默认用内存映射；可通过 `OPENCODE_SESSION_STORE_PATH` 启用简单文件持久化（适合 demo/单实例），生产环境建议用 DB/KV。
- 部分“回贴/发消息”失败会被 best-effort 忽略以保证主流程；可设置 `OPENCODE_BOT_VERBOSE=1`（或 `DEBUG=1`）输出被忽略的错误日志。
- 为避免路径误用，session store 支持设置根目录：`OPENCODE_SESSION_STORE_ROOT`（store path 必须在该目录下）。
- `OPENCODE_SESSION_STORE_ROOT` 是 best-effort 安全带而不是沙箱；仍应避免把不可信输入直接拼接到 store path，并避免用 symlink 等方式把路径指向 rootDir 之外。
- 对于飞书/钉钉/企微等平台：**“群机器人 webhook”通常只能发消息**，想做“交互式 bot”需要使用事件订阅/Stream/回调机制。
