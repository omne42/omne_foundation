# Event

```rust,no_run,edition2024
# extern crate notify_kit;
use notify_kit::{Event, Severity};

let event = Event::new("turn_completed", Severity::Success, "done")
    .with_body("all good")
    .with_tag("thread_id", "t1");
```

如果调用方已经持有 `structured-text-kit::StructuredText`，也可以显式保留结构化文本边界：

```rust,no_run,edition2024
# extern crate notify_kit;
# extern crate structured_text_kit;
use notify_kit::{Event, Severity};
use structured_text_kit::structured_text;

let event = Event::new_structured(
    "turn_completed",
    Severity::Success,
    structured_text!("notify.turn_completed.title", "repo" => "omne"),
)
.with_body_text(structured_text!(
    "notify.turn_completed.body",
    "step" => "review"
))
.with_tag_text("thread_id", structured_text!("notify.tag.thread_id", "value" => "t1"));
```

## 访问约定（建议）

- `kind`：推荐使用 `snake_case`，并保持稳定（便于过滤与统计）
- `title()`：一句话总结
- `body()`：可放更长的上下文（可为空）
- `tags()`：放结构化信息，便于 sink 以不同方式呈现

结构化语义通过 accessor 暴露：

- `title_text()`
- `body_text()`
- `tag_texts()`

plain-text sink 通过 `title()` / `body()` / `tags()` 读取当前可渲染的纯文本视图：

- freeform 文本会直接作为 plain-text 视图返回
- structured-only `CatalogText` 会保留在 `*_text()` 访问面里，不会自动泄漏成 `code {args}` 诊断串
- 如果调用方先通过 `Event::new` / `with_body` / `with_tag` 提供了 plain fallback，后续再写入 catalog text 时，这层用户可见文本会继续保留给纯文本 sinks

## 组合建议

一个实用的习惯是：

- `title()`：永远保持“一行可读”
- `body()`：放更长的细节（例如错误堆栈、上下文摘要）
- `tags()`：放结构化字段（例如 `thread_id`、`repo`、`step`、`elapsed_ms`）

## Severity

- `Info`：一般信息
- `Success`：成功完成
- `Warning`：需要关注但不致命
- `Error`：失败或需要立即处理
