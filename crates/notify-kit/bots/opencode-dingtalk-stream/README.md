# opencode-dingtalk-stream

一个最小可用的钉钉 Stream Mode 交互式 bot 示例，用于桥接 OpenCode 会话：

- 会话（sessionWebhook）→ session（首次消息创建会话并回贴分享链接）
- 群聊内持续对话（把用户消息发给 session.prompt）
- tool 完成时在群里提示（通过 event.subscribe 监听 `message.part.updated`）
- `/test` 命令回显

## 依赖

- Node.js 18+（建议 20+）
- 一个启用 Stream Mode 的钉钉机器人（应用）

## 配置

环境变量：

- `DINGTALK_CLIENT_ID`
- `DINGTALK_CLIENT_SECRET`

## 运行

```bash
cd bots/opencode-dingtalk-stream
npm install
npm start
```

## 说明

- 该 bot 会在本地启动一个 OpenCode server（端口随机）。默认在内存中维护 sessionWebhook → session 的映射；如设置 `OPENCODE_SESSION_STORE_PATH`（例如 `.opencode/sessions.json`），会把映射持久化到文件，重启后可恢复（可选：用 `OPENCODE_SESSION_STORE_ROOT` 限制 store 路径根目录）。
