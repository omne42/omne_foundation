# Event

```rust,no_run,edition2024
# extern crate notify_kit;
use notify_kit::{Event, Severity};

let event = Event::new("turn_completed", Severity::Success, "done")
    .with_body("all good")
    .with_tag("thread_id", "t1");
```

`notify-kit` 的事件边界现在只承诺纯字符串投递视图：

- `title`
- `body`
- `tags`

这样做是为了让所有 text/webhook sinks 都消费同一份真实消息文本，而不是把还没经过 renderer 的 catalog text 静默压成诊断串。

## 字段约定（建议）

- `kind`：推荐使用 `snake_case`，并保持稳定（便于过滤与统计）
- `title`：一句话总结
- `body`：可放更长的上下文（可为空）
- `tags`：放结构化信息，便于 sink 以不同方式呈现

兼容约束：

- `Event::new` / `with_body` / `with_tag` 是唯一公开 builder
- 如果调用方持有更高层的 i18n / structured-text 语义，应先在自己的 integration 层完成渲染，再把最终用户可见文本交给 `notify-kit`
- `notify-kit` 不再假装替调用方保留一个“尚未渲染但可以安全发送”的结构化文本镜像

## 组合建议

一个实用的习惯是：

- `title`：永远保持“一行可读”
- `body`：放更长的细节（例如错误堆栈、上下文摘要）
- `tags`：放结构化字段（例如 `thread_id`、`repo`、`step`、`elapsed_ms`）

## Severity

- `Info`：一般信息
- `Success`：成功完成
- `Warning`：需要关注但不致命
- `Error`：失败或需要立即处理
