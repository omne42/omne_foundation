# Event

```rust,no_run,edition2024
# extern crate notify_kit;
use notify_kit::{Event, Severity};

let event = Event::new("turn_completed", Severity::Success, "done")
    .with_body("all good")
    .with_tag("thread_id", "t1");
```

## 字段约定（建议）

- `kind`：推荐使用 `snake_case`，并保持稳定（便于过滤与统计）
- `title`：一句话总结
- `body`：可放更长的上下文（可为空）
- `tags`：放结构化信息，便于 sink 以不同方式呈现

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
