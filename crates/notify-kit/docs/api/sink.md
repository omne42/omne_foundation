# Sink

`Sink` 是你扩展通知渠道的核心抽象：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::future::Future;
use std::pin::Pin;

use notify_kit::Event;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, notify_kit::Result<()>>;
}
```

## 设计要点

- `send(&Event)` 是异步的；使用 boxed future 可以避免额外宏依赖（例如 `async-trait`）。
- `Hub` 会并发调用每个 sink，并对每个 sink 做超时包裹（`per_sink_timeout`）。

## 实现模板

```rust,no_run,edition2024
# extern crate notify_kit;
use std::future::Future;
use std::pin::Pin;

use notify_kit::{Event, Sink};

pub struct MySink;

impl Sink for MySink {
    fn name(&self) -> &'static str {
        "my_sink"
    }

    fn send<'a>(
        &'a self,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = notify_kit::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let _ = event;
            Ok(())
        })
    }
}
```

## 最佳实践

- `name()`：用于日志与聚合错误信息，保持稳定且可读。
- `send()`：避免阻塞；优先使用异步 IO（或把阻塞工作转移到专用线程池）。
- 超时：`Hub` 会做兜底超时；如果你的 sink 需要更细粒度控制，可以在 sink 内部再做一次超时/重试。
- 取消：`Hub` 的超时会 drop 你的 future；请确保 drop 不会泄露敏感信息或导致资源泄露。
- 错误信息：避免泄露敏感信息（token/webhook/用户数据）；`Debug` 输出建议默认脱敏。
