# model-assets-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`model-assets-kit` 负责本地 / 远程 AI 模型资产的共享 manifest、来源、能力、安装状态和本地引用类型。

它解决的问题不是“怎么下载文件”或“怎么执行本地 Whisper”，而是让语音转写、本地模型、桌面助手等项目共享同一套模型资产边界。

## 边界

负责：

- 模型 manifest
- 模型 checksum（SHA-1 / SHA-256）描述
- 模型 family / format
- 模型来源描述
- 模型能力描述
- 本地模型引用
- 安装请求和安装进度状态
- 本地 runtime backend 标识

不负责：

- HTTP 下载实现
- Hugging Face client 实现
- sha256 计算实现
- 归档解压、原子替换或文件锁
- sidecar 进程执行
- 产品级默认模型和下载 UI

## 范围

覆盖：

- Whisper GGML/GGUF 这类本地语音模型资产
- 官方 `whisper.cpp` GGML 模型 manifest catalog，包括 standard、English-only、tinydiarize、q5/q8 量化模型
- Hugging Face / HTTPS / 本地导入等来源描述
- 可序列化 DTO，便于跨 Tauri、CLI 或服务边界传递
- pending / downloading / verifying / installing / ready / failed / cancelled 等安装状态

不覆盖：

- 模型仓库 API client
- 执行 runtime adapter
- GPU backend 探测
- 模型许可证合规决策

## 结构设计

- `src/lib.rs`
  - `ModelManifest`
  - `ModelChecksum`
  - `ModelFamily`
  - `ModelFormat`
  - `ModelSource`
  - `ModelCapability`
  - `LocalModelRef`
  - `ModelInstallRequest`
  - `ModelInstallProgress`
  - `ModelInstallStatus`
  - `LocalModelRuntimeBackend`
  - `WhisperCppModelSpec`
  - `WHISPER_CPP_MODEL_SPECS`
  - `whisper_cpp_builtin_model_manifests`
  - `infer_whisper_cpp_model_id`

## 与其他 crate 的关系

- 当前不依赖其他 foundation crate。
- 下载、校验、归档和原子安装应复用 `omne-runtime` 原语。
- provider token 解析应复用 [`secret-kit`](../secret-kit/README.md)。
- 语音转写请求和结果应继续由 [`speech-transcription-kit`](../speech-transcription-kit/README.md) 表达。
- 本地 Whisper Rust 执行适配由 [`speech-whisper-kit`](../speech-whisper-kit/README.md) 表达。
