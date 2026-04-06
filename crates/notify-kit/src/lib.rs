#![forbid(unsafe_code)]

/// Convenience env wiring helpers.
///
/// This module stays separate from the crate root Hub/Sink surface on purpose: `notify-kit` does
/// not define a mandatory env protocol, and projects that need product-specific configuration
/// should keep that wiring in their own integration layer.
pub mod env;
mod error;
mod event;
mod hub;
mod log;
mod sinks;

pub use crate::error::{Error, ErrorKind, SinkFailure};
pub type Result<T> = std::result::Result<T, Error>;

pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, HubLimits, TryNotifyError};
pub use crate::sinks::{
    BarkConfig, BarkSink, DingTalkWebhookConfig, DingTalkWebhookSink, DiscordWebhookConfig,
    DiscordWebhookSink, FeishuWebhookConfig, FeishuWebhookSink, GenericWebhookConfig,
    GenericWebhookSink, GitHubCommentConfig, GitHubCommentSink, PushPlusConfig, PushPlusSink,
    ServerChanConfig, ServerChanSink, Sink, SlackWebhookConfig, SlackWebhookSink, SoundConfig,
    SoundSink, TelegramBotConfig, TelegramBotSink, WeComWebhookConfig, WeComWebhookSink,
};
pub(crate) use secret_kit::SecretString;
