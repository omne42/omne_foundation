# speech-transcription-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`speech-transcription-kit` 负责语音转写请求、响应、结果和 provenance 的跨仓库基础表示。

它沉淀的是“音频输入、转写选项、输出格式和结果文本”这类稳定 DTO，不承载任何 provider runtime。

## 边界

负责：

- 内联音频输入引用
- 转写模型、语言、prompt、输出格式、temperature 选项
- 转写响应文本和 provider metadata 容器
- 转写结果片段、provider provenance 和错误分类 DTO

不负责：

- OpenAI / provider-specific multipart transport
- provider routing
- 音频文件读取、缓存或转码
- speech synthesis

## 范围

覆盖：

- `TranscriptionAudioSource`
- `TranscriptionResponseFormat`
- `TranscriptionOptions`
- `TranscriptionRequest`
- `TranscriptionResponse`
- `TranscriptionResult`
- `TranscriptionSegment`
- `TranscriptionProviderProvenance`
- `TranscriptionError`
- `TranscriptionErrorKind`
