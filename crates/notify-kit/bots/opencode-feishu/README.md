# opencode-feishu

一个最小可用的飞书（Feishu/Lark）交互式 bot 示例，用于桥接 OpenCode 会话：

- 群聊（chat）→ session（首次消息创建会话并回贴分享链接）
- 群聊内持续对话（把用户消息发给 session.prompt）
- tool 完成时在群里提示（通过 event.subscribe 监听 `message.part.updated`）
- `/test` 命令回显

## 依赖

- Node.js 18+（建议 20+）
- 一个飞书开放平台应用（机器人），启用事件订阅

## 配置

环境变量：

- `FEISHU_APP_ID`
- `FEISHU_APP_SECRET`
- `FEISHU_VERIFICATION_TOKEN`
- `FEISHU_ENCRYPT_KEY`（可选；如果你的事件订阅启用了加密）
- `PORT`（可选；默认 `3000`）

在飞书开放平台后台配置：

- 事件订阅回调地址：`https://<your-host>/webhook/event`
- 订阅事件：`im.message.receive_v1`

> 注意：该示例通过 HTTP 接收事件回调，你需要把服务暴露到公网（或用内网穿透）。

## 运行

```bash
cd bots/opencode-feishu
npm install
npm start
```

## 说明

- 该 bot 会在本地启动一个 OpenCode server（端口随机）。默认在内存中维护 chat → session 的映射；如设置 `OPENCODE_SESSION_STORE_PATH`（例如 `.opencode/sessions.json`），会把映射持久化到文件，重启后可恢复（可选：用 `OPENCODE_SESSION_STORE_ROOT` 限制 store 路径根目录）。
