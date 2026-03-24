# opencode-slack

一个最小可用的 Slack 交互式 bot（Socket Mode），实现 OpenCode `packages/slack` 的核心行为：

- Slack thread → session（首次消息创建会话并回贴分享链接）
- thread 内持续对话（把用户消息发给 session.prompt）
- tool 完成时在 thread 内提示（通过 event.subscribe 监听 `message.part.updated`）
- `/test` 命令回显

## 依赖

- Node.js 18+（建议 20+）
- 一个启用 Socket Mode 的 Slack App

## 配置

在 Slack App 中配置并安装到 workspace，至少需要：

- Socket Mode enabled
- OAuth scopes：
  - `chat:write`
  - `app_mentions:read`
  - `channels:history`
  - `groups:history`

环境变量（放到 `.env` 或你的部署系统中）：

- `SLACK_BOT_TOKEN`
- `SLACK_SIGNING_SECRET`
- `SLACK_APP_TOKEN`

## 运行

```bash
cd bots/opencode-slack
npm install
npm start
```

## 说明

- 该 bot 会在本地启动一个 OpenCode server（端口随机）。默认在内存中维护 thread → session 的映射；如设置 `OPENCODE_SESSION_STORE_PATH`（例如 `.opencode/sessions.json`），会把映射持久化到文件，重启后可恢复（可选：用 `OPENCODE_SESSION_STORE_ROOT` 限制 store 路径根目录）。
- 如需把事件/通知转发到飞书/钉钉/企微等平台，建议由你的上层系统订阅/采集事件后调用 `notify-kit` 的对应 sink 发送。
