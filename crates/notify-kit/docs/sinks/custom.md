# 自定义 Sink

实现 `Sink` trait 即可扩展任何通知渠道。

`Sink::send` 的返回值是一个 boxed future。你不需要引用 `notify-kit` 内部的别名，
直接返回 `Pin<Box<dyn Future<...>>>` 即可。

## 一个最小例子：打印到 stderr

```rust,no_run,edition2024
# extern crate notify_kit;
use std::future::Future;
use std::pin::Pin;

use notify_kit::{Event, Sink};

#[derive(Debug)]
struct StderrSink;

impl Sink for StderrSink {
    fn name(&self) -> &'static str {
        "stderr"
    }

    fn send<'a>(
        &'a self,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = notify_kit::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            eprintln!("[{}] {}", event.kind, event.title);
            Ok(())
        })
    }
}
```

## 常见实现模式

- **Webhook / HTTP**：用 `reqwest` 发送请求；禁用重定向、限制域名、设置 timeout。
- **消息队列**：把 `Event` 序列化后投递到 MQ，再由异步 worker 批量发送到外部系统。
- **节流/合并**：高频事件（例如进度更新）可以在 sink 内按时间窗口合并，减少噪音。
- **幂等/去重**：用 `(kind, tags...)` 生成 key，短时间内去重，避免重复通知。

## 什么时候该做一个新的 crate？

如果你的 sink 依赖较重（SMTP/SDK/复杂鉴权），推荐放到独立 crate：

- `notify-kit` 保持轻量与通用
- 你的项目按需引入相关 sink crate

## 测试建议

自定义 sink 通常很适合用“收集器”来做单元测试：

- 在 sink 内部用 `Mutex<Vec<Event>>` 收集收到的事件
- 测试时调用 `hub.send(event).await`，断言收集到的内容
