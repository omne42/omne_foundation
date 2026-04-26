# redaction-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`redaction-kit` 负责通用观测与审计输出脱敏。

它解决的问题不是“某个产品记录哪些事件”，而是“JSON payload、命名字符串和 Prometheus label 在写入日志、指标或审计前如何按稳定规则脱敏”。

## 边界

负责：

- JSON key 名称脱敏
- JSON Pointer 定点脱敏
- 字符串 regex 脱敏
- URL query 参数脱敏
- Prometheus label value 脱敏
- 基于 sink 与 payload identity 的稳定采样判断

不负责：

- 产品级日志 schema
- 指标命名
- 审计事件含义
- provider、gateway 或业务策略

## 范围

覆盖：

- `RedactionRules`
- `Redactor`
- `stable_sample_json_payload(...)`
- `validate_sample_rate(...)`

不覆盖：

- 文件内容 redaction pipeline
- 路径 deny glob
- 远程日志投递

