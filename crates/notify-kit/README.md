# notify-kit

源码入口：[`src/lib.rs`](./src/lib.rs)  
详细文档：[`docs/README.md`](./docs/README.md)

## 领域

`notify-kit` 负责通知事件分发。

它把统一事件模型路由到多个 sink，并在不阻塞主流程的前提下处理并发发送和超时兜底。

## 边界

负责：

- 统一事件模型
- `Hub` 调度
- `Sink` 抽象
- 各通知渠道投递
- 并发发送和 per-sink timeout
- 错误聚合

不负责：

- 业务事件生成
- 可靠消息队列
- 复杂重试与持久化投递策略

## 范围

覆盖：

- `Event`
- `Severity`
- `Hub`
- `HubLimits`
- 一组内置 webhook / bot sink
- 标准环境变量接线 helper

不覆盖：

- 强投递保证
- 消息幂等与审计
- 高吞吐有序通知系统
- locale-aware structured-text 渲染

## 结构设计

- `notify_kit::core`
  - canonical 的窄通知边界；包含 `Hub`、`Event`、`Error` 和 `Sink`
- `notify_kit::builtin`
  - 内置 provider sinks 的适配层；crate root 继续保留 re-export 仅为兼容
- `notify_kit::builtin::env`
  - 标准 `NOTIFY_*` bootstrap helper 的 canonical 入口；`notify_kit::env` 仅保留为兼容出口，并建议迁移到这里
- `src/event.rs`
  - 统一事件模型
- `src/hub.rs`
  - hub 配置、限制、并发发送与错误聚合
- `src/sinks/mod.rs`
  - sink trait、条件导出和 selective feature 入口
- `src/sinks/feishu/`
  - Feishu webhook 适配内部再按 webhook send、payload 和 media support 拆开；图片加载、tenant token cache 与上传编排属于内部 media 子组件，不继续平铺在 sink 本体
- `src/env.rs`
  - convenience helper 的兼容 shim；不属于核心协议边界
- `bots/`
  - 上层集成示例，不是核心 Rust API

## StructuredText Contract

`Event` 可以携带 `StructuredText`，但这不等于内置 sinks 会负责做 locale-aware 渲染。

- freeform 文本会按原文透传到 sink
- 非 freeform `StructuredText` 会在 sink-facing 投影里降级成稳定字符串
- 如果你需要最终用户可见的本地化文案，应先在上层完成渲染，再传给 `Event::new(...)` / `with_body(...)` / `with_tag(...)`

这条边界是刻意保持窄的：`notify-kit` 负责通知分发，不接管 `i18n-kit` 的目录语义或 runtime 渲染策略

## 与其他 crate 的关系

- 依赖 `log-kit` 统一关键 warning 的稳定日志 code 与字段
- 详细用法和 sink 专题文档放在 crate 自己的 `docs/`

## Feishu Boundary

- `FeishuWebhookSink` 的 canonical 责任仍是 webhook 发送、签名和 payload 组装。
- markdown image upload 需要的远程下载、本地文件白名单、tenant token cache 与上传编排，已经收口到内部 media support 子组件，而不是继续扩张 `Sink` trait。
- 如果需要 construction-time 的公网 IP 预检，优先使用 async strict constructor；sync strict constructor 仅保留为兼容入口。

## 接入边界

`notify-kit` 当前不是独立的 crates.io 发布契约。它依赖的 foundation crate 仍按 workspace 一起演进，因此当前接入方式以 Git / monorepo 为准，不应假设 crates.io 依赖链已经稳定。

如果你要跨仓复用，优先依赖对应 Git revision；如果你在 monorepo 内接入，直接使用 workspace path 即可。

默认 feature 仍继续启用 `all-sinks` 以保持兼容；如果调用方想主动缩小依赖面，可以显式关闭默认 feature 再按需开启具体 sink feature。

## 进一步阅读

- 入门与最小示例：[`docs/README.md`](./docs/README.md)、[`docs/getting-started.md`](./docs/getting-started.md)
- 设计与并发/错误边界：[`docs/design.md`](./docs/design.md)
- integration layer、`NOTIFY_*` helper 与配置建议：[`docs/integration.md`](./docs/integration.md)
- features、timeout、StructuredText 与常见排错：[`docs/faq.md`](./docs/faq.md)
- sink 专题与安全说明：[`docs/sinks/README.md`](./docs/sinks/README.md)、[`docs/security.md`](./docs/security.md)
- bots 与上层集成示例：[`bots/README.md`](./bots/README.md)
- Rustdoc：`cargo doc -p notify-kit --open`

## 开发检查

```bash
CARGO_NET_OFFLINE=true ./scripts/gate.sh
```
