#![forbid(unsafe_code)]

pub mod builtin;
pub mod core;
pub mod env;
mod error;
mod event;
mod hub;
mod log;
mod sinks;

#[cfg(any(not(feature = "selective-sinks"), feature = "bark"))]
pub use crate::builtin::{BarkConfig, BarkSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "dingtalk"))]
pub use crate::builtin::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "discord"))]
pub use crate::builtin::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "feishu"))]
pub use crate::builtin::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "generic-webhook"))]
pub use crate::builtin::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "github"))]
pub use crate::builtin::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "pushplus"))]
pub use crate::builtin::{PushPlusConfig, PushPlusSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "serverchan"))]
pub use crate::builtin::{ServerChanConfig, ServerChanSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "slack"))]
pub use crate::builtin::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "sound"))]
pub use crate::builtin::{SoundConfig, SoundSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "telegram"))]
pub use crate::builtin::{TelegramBotConfig, TelegramBotSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "wecom"))]
pub use crate::builtin::{WeComWebhookConfig, WeComWebhookSink};
pub use crate::core::{Error, ErrorKind, Event, Hub, HubConfig, HubLimits, Result, Severity};
pub use crate::core::{Sink, SinkFailure, TryNotifyError};
pub use secret_kit::SecretString;
