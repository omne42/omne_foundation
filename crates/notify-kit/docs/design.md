# 设计说明

## 目标

- **轻量**：作为基础库，依赖尽量少、集成成本低
- **可扩展**：通过 `Sink` 抽象接入任意通知渠道
- **不阻塞**：默认 `notify()` 不影响主流程；`send()` 也有 per-sink timeout 兜底
- **安全意识**：对 webhook 做限制；避免在日志中泄露敏感信息
- **边界清晰**：核心 `Hub` / `Event` / `Sink` 抽象不再强制携带全部 vendor sink 依赖面

## 非目标

- 不把“统一的环境变量协议”作为核心抽象（交由上层 integration 层决定）
- 不追求复杂的重试/队列/投递保证（可在上层或自定义 sink 中实现）

补充说明：

- 库中提供的 `notify_kit::integration::standard_env::build_hub_from_standard_env(...)` / `notify_kit::integration::standard_env::StandardEnvHubOptions` 只是 convenience helper，用于快速接线或复用一套简单约定。
- 它们不改变整体分层：配置协议依然属于 integration layer，而不是 `notify-kit` 的核心职责。
- 公开入口统一使用 `notify_kit::integration::standard_env::...` 路径，不在 crate root 再叠兼容入口。
- 默认 features 仍保留当前内置 sinks；需要更窄依赖面时，可通过 `default-features = false` 只拿 core，再按需打开 `sink-*` / `standard-env`。

## 并发模型

当 `Hub::send(event).await` 执行时：

- 对每个 sink 生成一个并发任务
- 每个任务都被 `tokio::time::timeout(per_sink_timeout, ...)` 包裹
- 所有结果被 join 并聚合错误，最终以 `notify_kit::Error` 返回
