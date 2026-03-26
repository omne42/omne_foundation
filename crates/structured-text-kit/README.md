# structured-text-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`structured-text-kit` 负责结构化用户文本原语。

它解决的问题不是“消息系统怎么设计”，而是“一个用户可见文本，如何以稳定、可校验、可嵌套的结构被构造和传递”。

这里的文本只有两类：

- `CatalogText`：由稳定 `code` 和命名参数组成
- `StructuredText::freeform(...)`：已经是最终用户可见原文的自由格式文本

## 边界

负责：

- 定义 `StructuredText` 与 `CatalogText`
- 定义参数和值类型
- 定义嵌套结构化文本规则
- 在编译期和运行期校验 code、参数名和嵌套深度
- 提供稳定的显示与序列化基础

不负责：

- locale 选择与 i18n 渲染
- 跨语言 DTO / schema 输出
- IM 消息、事件消息、进程通信消息
- 业务错误分类或错误恢复流程

## 范围

覆盖：

- 字面量宏构造和运行期安全构造
- 参数唯一性和稳定排序
- 文本、布尔、有符号整数、无符号整数、嵌套文本参数
- `Display`、诊断显示和 serde 序列化

不覆盖：

- 上层产品如何命名 catalog code
- 复杂模板语言
- 翻译目录查找和 fallback 策略

## 结构设计

- `src/lib.rs`
  - 对外导出核心类型和构造规则
- `src/text.rs`
  - `StructuredText`、`CatalogText`、参数与只读引用视图
- `src/scalar.rs`
  - 标量参数值约束
- `src/validation.rs`
  - code、参数名、嵌套深度校验
- `src/macros.rs`
  - `structured_text!` 与 `try_structured_text!`
- `src/render.rs`
  - 文本显示与诊断显示
- `src/serialize.rs`
  - 稳定机器可读序列化

## 与其他 crate 的关系

- 被 [`error-kit`](../error-kit/README.md) 用作结构化错误文本载体
- 被 [`log-kit`](../log-kit/README.md) 用作结构化日志文本载体
- 被 [`structured-text-protocol`](../structured-text-protocol/README.md) 用作协议源模型
- 被 [`i18n-kit`](../i18n-kit/README.md) 消费用于最终文本渲染
- 被 [`secret-kit`](../secret-kit/README.md) 用作结构化错误文本表达
