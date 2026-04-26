# desktop-input-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`desktop-input-kit` 负责桌面输入触发和文本交付的共享契约。

它解决的问题不是“怎么注册 Tauri 全局快捷键”或“怎么在 macOS 注入文本”，而是让语音输入、桌面助手和其他桌面工具共享同一套触发、交付目标、权限和错误模型。

## 边界

负责：

- 输入触发来源
- 输入触发事件
- 语音唤醒触发设置和事件
- 文本交付目标
- 文本交付请求和结果
- 桌面能力权限说明
- 快捷键、剪贴板、辅助功能、输入模拟等错误分类

不负责：

- Tauri plugin 接线
- 系统托盘实现
- 全局快捷键注册
- 剪贴板读写实现
- 直接输入 / 光标注入实现
- 平台级权限引导 UI
- 产品默认快捷键和输出策略

## 范围

覆盖：

- window button、global shortcut、tray menu、external command 等触发来源
- voice activation 的 wake phrase、检测引擎选择、最短唤醒语音、探测冷却、静音阈值和最长听写限制 DTO
- clipboard、in-app draft、direct insert 等文本交付目标
- shortcut conflict、clipboard unavailable、accessibility missing 等稳定错误分类
- 可序列化 DTO，便于跨 Tauri、CLI 或服务边界传递

不覆盖：

- Tauri global-shortcut adapter
- Tauri tray adapter
- wake word / keyword spotting engine adapter
- Web SpeechRecognition adapter
- Tauri clipboard-manager adapter
- enigo 或其他输入模拟 adapter
- macOS Accessibility / Windows SendInput / Linux X11/Wayland 具体实现

## 结构设计

- `src/lib.rs`
  - `InputTrigger`
  - `InputTriggerKind`
  - `InputTriggerEvent`
  - `VoiceActivationSettings`
  - `VoiceActivationEngine`
  - `VoiceActivationEvent`
  - `TextDeliveryTarget`
  - `TextDeliveryRequest`
  - `TextDeliveryResult`
  - `DesktopPermission`
  - `DesktopPermissionKind`
  - `DesktopInputError`
  - `DesktopInputErrorKind`

## 与其他 crate 的关系

- 当前不依赖其他 foundation crate。
- 音频采集和转写状态由语音领域 crate 表达，不放进本 crate。
- 真实平台接线应留在桌面产品层或后续 adapter crate。
