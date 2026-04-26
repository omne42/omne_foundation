# speech-transcription-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`speech-transcription-kit` 负责语音转写领域的共享请求、音频资产引用、provider 能力、转写 job、结果、状态、错误和 provider provenance 类型。

它解决的问题不是“哪个产品何时录音”，也不是“怎么调用某个 provider”，而是多个语音输入、音频处理和本地模型项目共享同一套转写边界。

## 边界

负责：

- 转写输入引用
- 音频资产引用（复用并 re-export `audio-media-kit::AudioAssetRef`）
- 转写选项
- provider / model 选择
- provider registry / catalog
- provider / model 能力描述
- provider 默认模型建议
- 转写结果和片段
- 转写 job 快照
- 转写 job 状态
- 转写错误分类和重试提示
- provider provenance

不负责：

- 音频采集
- 音频解码、重采样或格式转换
- HTTP 调用
- 本地模型下载和执行
- 产品级历史 UI 或默认策略

## 范围

覆盖：

- 本地文件型音频输入引用
- OpenAI-compatible 与本地模型都能复用的转写请求模型
- provider 能力：语言、提示词、时间戳、翻译、VAD、流式结果、本地模型执行
- 统一错误：认证失败、限流、模型缺失、音频格式不支持、超时、provider 返回错误等
- 简单文本结果和可选时间片段
- 可序列化 DTO，便于跨 Tauri、CLI 或服务边界传递

不覆盖：

- provider SDK
- prompt 模板
- VAD pipeline
- 流式 token / partial transcript 协议

## 结构设计

- `src/lib.rs`
  - `TranscriptionAudioSource`
  - `AudioAssetRef`
  - `TranscriptionOptions`
  - `TranscriptionProviderSelection`
  - `TranscriptionProviderRegistry`
  - `TranscriptionProviderDescriptor`
  - `TranscriptionProviderKind`
  - `TranscriptionModelDescriptor`
  - `TranscriptionProviderCapability`
  - `TranscriptionRequest`
  - `TranscriptionResult`
  - `TranscriptionSegment`
  - `TranscriptionJob`
  - `TranscriptionJobStatus`
  - `TranscriptionError`
  - `TranscriptionErrorKind`
  - `TranscriptionProviderProvenance`

## 与其他 crate 的关系

- 依赖 [`audio-media-kit`](../audio-media-kit/README.md) 的 `AudioAssetRef`，并从本 crate re-export 以保持转写调用方入口稳定。
- HTTP provider 实现应复用 [`http-kit`](../http-kit/README.md)。
- provider token 解析应复用 [`secret-kit`](../secret-kit/README.md)。
- 本地模型资产和执行边界后续应与 `omne-runtime` 原语配合，而不是放进本 crate。
