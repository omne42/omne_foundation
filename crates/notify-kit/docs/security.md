# 安全

## 外部命令执行（SoundSink）

`SoundConfig.command_argv` 会执行外部命令：

- 需要启用 crate feature：`notify-kit/sound-command`
- 仅应由**本机受信任配置**提供
- 不要把不可信数据拼接到 argv（避免命令执行风险）

## Webhook（Feishu/Slack/Discord/钉钉/企微）

Webhook URL 属于敏感信息：

- 不要写入日志/错误信息/Debug 输出
- 使用配置系统安全存储（例如 secrets manager / 环境变量注入）
- 本库对 URL 做了 scheme/host/port/credentials 限制以降低 SSRF 风险

目前内置的 webhook sinks 允许的 host（精确匹配）：

- Feishu：`open.feishu.cn` / `open.larksuite.com`
- Slack：`hooks.slack.com`
- Discord：`discord.com` / `discordapp.com`
- 钉钉：`oapi.dingtalk.com`
- 企业微信：`qyapi.weixin.qq.com`
- Telegram：固定为 `api.telegram.org`

### 为什么要限制 host / 禁用重定向？

Webhook 发送本质是“服务端发起 HTTP 请求”。如果 URL 可被不可信输入影响，会引入 SSRF 风险。

本库的策略是：

- **允许的域名做 allow-list**（只放行官方 webhook 域名）
- **禁用重定向**（避免被 30x 绕过 allow-list）
- **校验 URL path 前缀**（避免误配到同域其它 endpoint）
- **错误信息保持低敏感**（不输出 body、不输出完整 URL）

## GenericWebhookSink（通用 webhook）

`GenericWebhookSink` 可以向任意 HTTPS URL 发送 JSON（如果不设置 `allowed_hosts`），因此当 URL 可被不可信输入影响时，依然存在 SSRF 风险（即使做了公网 IP 校验）。

建议：

- 尽量配置 `allowed_hosts` + `path_prefix`（把它们视为安全边界）
- 优先使用 `GenericWebhookSink::new_strict`（强制 `allowed_hosts`/`path_prefix`，且禁止关闭公网 IP 校验）
- 不要把 webhook URL 当作用户输入/可回显的字段

### DNS 解析结果必须是公网 IP（默认启用）

为降低 DNS 污染 / DNS rebinding / 内网解析等风险，内置 HTTP sinks 默认会在发送前做一次 DNS 解析校验：

- 若解析到私网/loopback/link-local，会拒绝发送
- 校验通过后，会把本次解析结果固定到该次请求的 DNS overrides（降低“校验后被 rebinding”的 TOCTOU 风险）
- 可通过各 sink 的 `with_public_ip_check(false)` 关闭（Feishu 的 `*_strict` 额外会在构造时也校验一次）

补充说明：

- “公网 IP”的判定是一个**保守**实现：除了私网/loopback/link-local，也会拒绝部分 RFC6890 中的特殊用途网段（例如 `192.0.0.0/24`、`192.88.99.0/24`）。
- 对 IPv6，会把 `::ffff:x.y.z.w`（IPv4-mapped）以及部分过渡机制前缀按**嵌入的 IPv4**再做一次判定（例如 NAT64 well-known prefix `64:ff9b::/96`、6to4 `2002::/16`）。
- 该判定不保证覆盖所有 IPv6 过渡/翻译前缀（例如自定义 NAT64 前缀）；在特殊网络环境可能出现误拒/漏判。把 URL 当作安全边界：优先用严格模式（allow-list + path 前缀），必要时只在受信任场景关闭公网 IP 校验。

注意：这是一个“更严格、更保守”的策略；在无网络/DNS 不可用时可能导致发送失败。`*_strict` 构造函数会把校验提前到构造阶段。

## GitHub API（GitHubCommentSink）

`GitHubCommentSink` 使用 GitHub token 调用 `api.github.com`：

- token 属于敏感信息：不要写入日志/错误信息/Debug 输出
- 建议用最小权限的 token（只授予目标仓库的必要写权限）

## 国内推送平台（ServerChan/PushPlus/Bark）

这些 sinks 通常需要 token / send_key / device_key：

- 属于敏感信息：不要写入日志/错误信息/Debug 输出
- 建议用最小权限/最小范围的 key（能发通知即可）

## DoS / 噪音控制

为了避免异常大消息或事件洪泛导致内存/网络放大，本库内置 sinks 会对内容做截断与上限：

- 文本总长度：按 sink 的 `max_chars`（或内置默认）截断并追加 `...`
- tags 数量与 tag key/value 长度：超出会截断/忽略（避免极端情况下构建超大 payload）
- JSON response：只会读取有限大小（默认 `16KiB`），并且错误信息不会包含 response body

另外，`Hub::notify` 内部有一个固定的 inflight 限制；超过上限会丢弃并 `warn`（避免无界 spawn 造成 DoS）。

## 错误信息与敏感数据

实现自定义 sink 时，建议：

- 错误信息避免包含 token、完整 URL、用户隐私数据
- `Debug` 输出对敏感字段做脱敏

## Event 内容也是敏感数据

`Event.title/body/tags` 由上层业务提供，可能包含：

- 用户输入
- 仓库路径/机器信息
- 错误堆栈

在实现 sink 时，建议把“对外发送的内容”当作需要审计的出口：

- 限制最大长度
- 对高敏感字段做删减/脱敏
- 必要时引入 allow-list（只发部分 kind）
