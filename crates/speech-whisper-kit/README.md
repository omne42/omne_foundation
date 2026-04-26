# speech-whisper-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`speech-whisper-kit` 负责本地 Whisper Rust 运行时适配边界。

它解决的问题不是“产品界面怎么选择模型”，也不是“模型怎么下载”，而是把可复用的本地 Whisper 执行逻辑从具体应用里抽出来，供 typemic 和后续需要语音转写的项目复用。

## 边界

负责：

- `whisper-rs` 本地模型执行适配
- 透传 `metal` / `cuda` feature 到 `whisper-rs`，让产品仓不直接依赖底层运行时 crate
- 通过 runtime no-follow 文件系统原语校验模型文件和 WAV 输入文件，拒绝 symlink 或非普通文件
- Whisper 所需 PCM WAV 输入校验
- 16 kHz mono/stereo PCM WAV 到 mono `f32` 样本转换
- 本地运行时配置、输入、输出和错误分类

不负责：

- 麦克风采集
- WebM/Opus 解码
- 模型下载、缓存、校验和安装
- GPU/Metal/CUDA 选择策略
- 产品级历史记录、设置页和快捷键

## 结构设计

- `WhisperRsRuntimeConfig`
- `WhisperTranscriptionInput`
- `WhisperTranscriptionOutput`
- `WhisperRuntimeError`
- `read_pcm_wav_as_mono_f32`
- `transcribe_wav`
- Cargo features: `metal`, `cuda`

## 与其他 crate 的关系

- 音频资产和前处理目标由 [`audio-media-kit`](../audio-media-kit/README.md) 表达。
- 转写请求、结果和 provider registry 由 [`speech-transcription-kit`](../speech-transcription-kit/README.md) 表达。
- 本地模型资产、来源和运行后端由 [`model-assets-kit`](../model-assets-kit/README.md) 表达。
- 文件系统 no-follow 打开与路径遍历由 `omne-runtime` 的 `omne-fs-primitives` 提供。
