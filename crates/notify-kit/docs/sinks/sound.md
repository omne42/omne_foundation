# SoundSink

`SoundSink` 提供两种模式：

1) 默认：向 stderr 写入终端 bell（`\u{0007}`）
2) 自定义：执行外部命令播放提示音

## 终端 bell（默认）

```rust,no_run,edition2024
# extern crate notify_kit;
use notify_kit::{SoundConfig, SoundSink};

let sink = SoundSink::new(SoundConfig { command_argv: None });
```

不同 `Severity` 会对应不同次数的 bell（用于区分提示强度）。

### 让 macOS / Windows “闪一下”

`SoundSink` 的默认行为是写入终端 bell（`\u{0007}`）。很多终端都支持把 bell 映射成“闪屏/标签闪烁/Dock 或任务栏提示”：

- macOS Terminal.app：Settings → Profiles → Advanced → Bell（Visual bell / Bounce Dock icon）
- iTerm2：Preferences → Profiles → Terminal → Notifications / Bells
- Windows Terminal：Settings（启用/配置 Visual bell，或由系统/终端实现提示行为）

`notify-kit` 只负责发出 bell；是否“闪”取决于你的终端/系统设置。

## 外部命令

> 需要启用 crate feature：`notify-kit/sound-command`。

```rust,no_run,edition2024
# extern crate notify_kit;
use notify_kit::{SoundConfig, SoundSink};

let sink = SoundSink::new(SoundConfig {
    command_argv: Some(vec!["afplay".into(), "/System/Library/Sounds/Glass.aiff".into()]),
});
```

### 多平台提示

外部命令完全由你决定，本库只负责 spawn：

- macOS：`afplay <path>`
- Linux（示例）：`paplay <path>` / `aplay <path>`
- Windows：可用任意你习惯的播放器/脚本（例如 powershell）

建议把命令作为**本机配置**管理，而不是写死在代码里。

注意：

- 外部命令会被 spawn，并在后台线程中 wait 回收进程（避免僵尸进程累积）。
- `command_argv` 属于**本机受信任配置**；不要把不可信输入拼到 argv 里。
- 如果你的配置可能来自远程/不可信来源（例如 bot、服务端动态配置），建议禁用外部命令模式，只使用默认 bell。
