#![forbid(unsafe_code)]

pub mod builtin;
pub mod core;
#[doc(hidden)]
pub mod env;
mod error;
mod event;
mod hub;
mod log;
mod secret;
mod sinks;

#[cfg(any(feature = "all-sinks", feature = "bark"))]
pub use crate::builtin::{BarkConfig, BarkSink};
#[cfg(any(feature = "all-sinks", feature = "dingtalk"))]
pub use crate::builtin::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "discord"))]
pub use crate::builtin::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "feishu"))]
pub use crate::builtin::{FeishuWebhookConfig, FeishuWebhookMediaConfig, FeishuWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
pub use crate::builtin::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "github"))]
pub use crate::builtin::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(any(feature = "all-sinks", feature = "pushplus"))]
pub use crate::builtin::{PushPlusConfig, PushPlusSink};
#[cfg(any(feature = "all-sinks", feature = "serverchan"))]
pub use crate::builtin::{ServerChanConfig, ServerChanSink};
#[cfg(any(feature = "all-sinks", feature = "slack"))]
pub use crate::builtin::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "sound"))]
pub use crate::builtin::{SoundConfig, SoundSink};
#[cfg(any(feature = "all-sinks", feature = "telegram"))]
pub use crate::builtin::{TelegramBotConfig, TelegramBotSink};
#[cfg(any(feature = "all-sinks", feature = "wecom"))]
pub use crate::builtin::{WeComWebhookConfig, WeComWebhookSink};
pub use crate::core::{Error, ErrorKind, Event, Hub, HubConfig, HubLimits, Result, Severity};
pub use crate::core::{Sink, SinkFailure, TryNotifyError};
pub use crate::secret::NotifySecret;
