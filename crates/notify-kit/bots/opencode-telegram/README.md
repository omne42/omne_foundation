# opencode-telegram

一个最小可用的 Telegram 交互式 bot 示例（long polling），用于桥接 OpenCode 会话：

- chat → session（首次消息创建会话并回贴分享链接）
- 持续对话（把用户消息发给 `session.prompt`）
- tool 完成时提示（通过 `event.subscribe` 监听 `message.part.updated`）
- `/test` 命令回显

## 依赖

- Node.js 18+（建议 20+）
- 一个 Telegram Bot（通过 BotFather 获取 token）

## 配置

环境变量：

- `TELEGRAM_BOT_TOKEN`

## 运行

```bash
cd bots/opencode-telegram
npm install
npm start
```

## 说明

- 该 bot 使用 long polling 拉取 `getUpdates`；更适合本地/轻量部署。
- 默认在内存中维护 chat → session 的映射；如设置 `OPENCODE_SESSION_STORE_PATH`（例如 `.opencode/sessions.json`），会把映射持久化到文件，重启后可恢复（可选：用 `OPENCODE_SESSION_STORE_ROOT` 限制 store 路径根目录）。
