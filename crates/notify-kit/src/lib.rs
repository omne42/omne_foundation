#![forbid(unsafe_code)]

pub mod env;
mod error;
mod event;
mod hub;
mod log;
mod sinks;

pub use crate::error::Error;
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
pub use secret_kit::SecretString;
