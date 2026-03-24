# 核心概念

## Event（事件）

`Event` 是你想“通知出去”的信息载体，包含：

- `kind`: 事件类型（字符串），用于过滤/路由（例如 `turn_completed`）
- `severity`: 严重级别（Info/Success/Warning/Error）
- `title`: 标题（必填）
- `body`: 可选正文
- `tags`: 结构化键值（例如 `thread_id=t1`、`repo=xxx`）

### `kind` 命名建议

- 推荐 `snake_case`（例如 `turn_completed`、`approval_requested`）
- 推荐稳定：把它当作“对外契约”，尽量不要频繁改名（便于订阅与统计）
- 推荐少而精：kind 过多会导致过滤难维护；把细节放到 `tags`/`body`

### `tags` 使用建议

- 放结构化、低敏感的信息（例如 id、类型、阶段）
- value 尽量短（更适合在 IM/通知卡片里展示）
- 不要把 token / webhook / 个人隐私信息塞进 tags

## Sink（通知渠道）

`Sink` 是一个抽象的“发送器”，负责把 `Event` 投递到某个外部系统/媒介。

- 内置：见 [内置 Sinks](sinks/README.md)
- 扩展：实现 `Sink` trait 即可接入任何渠道

## Hub（通知中心）

`Hub` 组合了：

- `enabled_kinds`: 可选 allow-list（只允许指定 `kind` 的事件）
- `per_sink_timeout`: 每个 sink 的超时上限（兜底避免卡住）
- `sinks`: 一组 `Arc<dyn Sink>`

发送逻辑：

- 逐个 sink 并发发送
- 每个 sink 单独超时
- 汇总错误（如有）
