# opencode-wecom

一个最小可用的企业微信（WeCom）交互式 bot 示例：通过“自建应用”的回调（Callback）接收消息，把用户消息桥接到 OpenCode session，并把回复发回企业微信。

- 用户消息 → session（首次消息创建会话并回贴分享链接）
- 持续对话（把用户消息发给 `session.prompt`）
- tool 完成时提示（通过 `event.subscribe` 监听 `message.part.updated`）
- `/test` 命令回显

## 依赖

- Node.js 18+（建议 20+）
- 一个企业微信自建应用（需要启用接收消息并配置回调 URL）

## 配置

环境变量：

- `WECOM_CORP_ID`：企业 ID（corpid）
- `WECOM_CORP_SECRET`：应用 secret
- `WECOM_AGENT_ID`：应用 AgentId
- `WECOM_TOKEN`：回调 Token（用于签名校验）
- `WECOM_ENCODING_AES_KEY`：回调 EncodingAESKey（43 位 base64 字符串）
- `PORT`（可选；默认 `3000`）

可选行为开关：

- `WECOM_SESSION_SCOPE`：`user`（默认）或 `chat`。决定“一个 session 绑定到用户还是群聊（如果回调里有 ChatId）”
- `WECOM_REPLY_TO`：`user`（默认）或 `chat`。如果你确认 `ChatId` 可用于 `appchat/send`，可切到 `chat` 把回复发到群聊。

## 在企业微信后台配置

在“应用管理 → 自建应用 → 接收消息”里配置：

- 回调 URL：`https://<your-host>/webhook/wecom`
- Token：填 `WECOM_TOKEN`
- EncodingAESKey：填 `WECOM_ENCODING_AES_KEY`

> 注意：企业微信回调通常需要公网可访问（或内网穿透）。

## 运行

```bash
cd bots/opencode-wecom
npm install
npm start
```

## 说明

- 默认在内存中维护映射；如设置 `OPENCODE_SESSION_STORE_PATH`（例如 `.opencode/sessions.json`），会把映射持久化到文件，重启后可恢复（可选：用 `OPENCODE_SESSION_STORE_ROOT` 限制 store 路径根目录）。
- 为降低重放风险，回调会做 timestamp 时间窗校验 + nonce 去重；当前去重缓存是内存级的（重启会清空）。
