# audio-input-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`audio-input-kit` 负责音频输入领域的共享设备、配置、session 状态、事件、错误类型和最小 native 麦克风采集 adapter。

它解决的问题不是“哪个产品什么时候开始录音”，也不是直接绑定 CPAL / Tauri / 浏览器 API，而是让桌面语音输入、语音助手和其他需要音频输入的项目共享同一套边界。

## 边界

负责：

- 音频输入 backend 标识
- 音频设备标识
- 音频帧格式和采集配置
- capture session 状态
- capture session 事件
- 音频输入错误分类
- CPAL native input device listing
- CPAL native mono `f32` input stream adapter

不负责：

- 音频解码、重采样或格式转换
- 录音文件写入
- 产品级快捷键、托盘或 UI 策略

## 范围

覆盖：

- Web MediaRecorder、CPAL native、system-audio 等 backend 都能复用的 DTO
- 可序列化事件，便于跨 Tauri、CLI 或服务边界传递
- 权限、设备不可用、配置不支持、backend 不可用等稳定错误分类
- 基于 CPAL 的默认麦克风输入流，输出 normalized mono `f32` frames

不覆盖：

- 浏览器 MediaRecorder adapter 实现
- 系统音频 tap / loopback 的平台实现
- 音频资产 manifest 或转写预处理 pipeline

## 结构设计

- `src/lib.rs`
  - `AudioInputBackend`
  - `AudioDeviceId`
  - `AudioSampleFormat`
  - `AudioFrameFormat`
  - `AudioInputConfig`
  - `CaptureSessionId`
  - `CaptureSessionStatus`
  - `CaptureEvent`
  - `AudioInputErrorKind`
  - `AudioInputRuntimeError`
  - `list_cpal_input_devices`
  - `start_cpal_mono_input_stream`

## 与其他 crate 的关系

- 当前不依赖其他 foundation crate。
- 采集后形成的音频资产引用应交给 [`audio-media-kit`](../audio-media-kit/README.md)，再由 [`speech-transcription-kit`](../speech-transcription-kit/README.md) 复用。
- 音频文件读写、校验和安装这类底层原语应复用 `omne-runtime`，不放进本 crate。
