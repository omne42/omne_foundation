# notify-kit

源码入口：[`crates/notify-kit/src/lib.rs`](../../crates/notify-kit/src/lib.rs)  
详细文档：[`crates/notify-kit/docs/README.md`](../../crates/notify-kit/docs/README.md)

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

## 结构设计

- `src/event.rs`
  - 统一事件模型
- `src/hub.rs`
  - hub 配置、限制、并发发送与错误聚合
- `src/sinks/mod.rs`
  - sink trait 与各实现导出
- `src/sinks/http/`
  - webhook 类 sink 的共享 HTTP 逻辑
- `src/env.rs`
  - convenience helper，不是核心协议边界
- `bots/`
  - 上层集成示例，不是核心 Rust API

## 与其他 crate 的关系

- 当前不依赖 workspace 内其他 crate
- 详细用法和 sink 专题文档放在 crate 自己的 `docs/`
