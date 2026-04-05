#![forbid(unsafe_code)]

mod error;
mod event;
mod hub;
#[cfg(feature = "standard-env")]
pub mod integration;
mod log;
mod secret;
mod sinks;

pub use crate::error::{Error, ErrorKind, SinkFailure};
pub type Result<T> = std::result::Result<T, Error>;

pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, HubLimits, TryNotifyError};
pub use crate::secret::SecretString;
pub use crate::sinks::Sink;
#[cfg(feature = "sink-bark")]
pub use crate::sinks::{BarkConfig, BarkSink};
#[cfg(feature = "sink-dingtalk")]
pub use crate::sinks::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(feature = "sink-discord")]
pub use crate::sinks::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(feature = "sink-feishu")]
pub use crate::sinks::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(feature = "sink-generic-webhook")]
pub use crate::sinks::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(feature = "sink-github")]
pub use crate::sinks::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(feature = "sink-pushplus")]
pub use crate::sinks::{PushPlusConfig, PushPlusSink};
#[cfg(feature = "sink-serverchan")]
pub use crate::sinks::{ServerChanConfig, ServerChanSink};
#[cfg(feature = "sink-slack")]
pub use crate::sinks::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(feature = "sink-sound")]
pub use crate::sinks::{SoundConfig, SoundSink};
#[cfg(feature = "sink-telegram")]
pub use crate::sinks::{TelegramBotConfig, TelegramBotSink};
#[cfg(feature = "sink-wecom")]
pub use crate::sinks::{WeComWebhookConfig, WeComWebhookSink};
