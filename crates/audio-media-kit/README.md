# audio-media-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`audio-media-kit` 负责音频资产、音频格式和转写前处理的共享边界。

它解决的问题不是“怎么调用 Symphonia 解码”或“怎么把 WAV 写到磁盘”，而是让语音输入、语音转写、本地模型和其他音频项目共享同一套资产元数据、处理预算、目标格式和 provenance 契约。

## 边界

负责：

- 音频资产引用
- 音频容器、codec 和媒体格式描述
- 转写前处理目标格式
- decode / resample / mixdown / normalize pipeline 步骤
- 有界处理预算
- 处理结果 provenance
- 音频媒体处理错误分类

不负责：

- 真实文件读写
- 真实音频解码
- 真实重采样或归一化
- WAV / WebM / MP3 编码实现
- sha256 计算实现
- 产品级录音、历史 UI 或保留策略

## 范围

覆盖：

- WebM / WAV / MP3 / FLAC / OGG / MP4 / raw PCM 等常见音频资产的共享描述
- 转写后端常见的 PCM WAV 目标输入约束
- 可序列化 DTO，便于跨 Tauri、CLI 或服务边界传递
- unsupported format、budget exceeded、decode failed 等稳定错误分类

不覆盖：

- Symphonia adapter 实现
- Hound adapter 实现
- Rubato adapter 实现
- VAD pipeline
- 转写 provider 请求和结果

## 结构设计

- `src/lib.rs`
  - `AudioAssetRef`
  - `AudioAsset`
  - `AudioContainerFormat`
  - `AudioCodec`
  - `AudioMediaFormat`
  - `AudioPreprocessTarget`
  - `AudioProcessingBudget`
  - `AudioProcessingStep`
  - `AudioPreprocessRequest`
  - `AudioPreprocessResult`
  - `AudioPreprocessProvenance`
  - `AudioMediaErrorKind`

## 与其他 crate 的关系

- 当前不依赖其他 foundation crate。
- 音频采集 session 和设备语义由 [`audio-input-kit`](../audio-input-kit/README.md) 表达。
- 语音转写请求和结果由 [`speech-transcription-kit`](../speech-transcription-kit/README.md) 表达，并复用本 crate 的 `AudioAssetRef`。
- 文件读写、sha256、原子替换和安装这类底层原语应复用 `omne-runtime`，不放进本 crate。
