# Sink

`Sink` 是你扩展通知渠道的核心抽象：

```rust,no_run,edition2024
# extern crate notify_kit;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use notify_kit::core::Event;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn label(&self) -> Cow<'_, str> { Cow::Borrowed(self.name()) }
    fn identity(&self) -> Cow<'_, str> { self.label() }
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, notify_kit::Result<()>>;
}
```

## 设计要点

- `send(&Event)` 是异步的；使用 boxed future 可以避免额外宏依赖（例如 `async-trait`）。
- `Hub` 会并发调用每个 sink，并对每个 sink 做超时包裹（`per_sink_timeout`）。

## 实现模板

```rust,no_run,edition2024
# extern crate notify_kit;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use notify_kit::core::{Event, Sink};

pub struct MySink;

impl Sink for MySink {
    fn name(&self) -> &'static str {
        "my_sink"
    }

    fn identity(&self) -> Cow<'_, str> {
        Cow::Borrowed("my_sink(primary)")
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

- `name()`：保留为 sink 类型名/兼容入口，适合返回稳定的静态类别名。
- `label()`：可选的人类可读标签；默认复用 `name()`。
- `identity()`：用于 Hub 内部记录与聚合错误中的 sink 实例标识；默认复用 `label()`。如果同类 sink 会并存，推荐显式提供稳定且可区分的 identity（例如 `github(repo=docs)`）。
- `send()`：避免阻塞；优先使用异步 IO（或把阻塞工作转移到专用线程池）。
- 超时：`Hub` 会做兜底超时；如果你的 sink 需要更细粒度控制，可以在 sink 内部再做一次超时/重试。
- 取消：`Hub` 的超时会 drop 你的 future；请确保 drop 不会泄露敏感信息或导致资源泄露。
- 错误信息：避免泄露敏感信息（token/webhook/用户数据）；`Debug` 输出建议默认脱敏。
