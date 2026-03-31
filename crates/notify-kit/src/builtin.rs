//! Built-in sink implementations.
//!
//! This namespace groups provider-specific adapters so callers that only need the
//! core notification boundary can stay on [`crate::core`].

#[cfg(any(feature = "all-sinks", feature = "bark"))]
pub use crate::sinks::{BarkConfig, BarkSink};
#[cfg(any(feature = "all-sinks", feature = "dingtalk"))]
pub use crate::sinks::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "discord"))]
pub use crate::sinks::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "feishu"))]
pub use crate::sinks::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
pub use crate::sinks::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "github"))]
pub use crate::sinks::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(any(feature = "all-sinks", feature = "pushplus"))]
pub use crate::sinks::{PushPlusConfig, PushPlusSink};
#[cfg(any(feature = "all-sinks", feature = "serverchan"))]
pub use crate::sinks::{ServerChanConfig, ServerChanSink};
#[cfg(any(feature = "all-sinks", feature = "slack"))]
pub use crate::sinks::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "sound"))]
pub use crate::sinks::{SoundConfig, SoundSink};
#[cfg(any(feature = "all-sinks", feature = "telegram"))]
pub use crate::sinks::{TelegramBotConfig, TelegramBotSink};
#[cfg(any(feature = "all-sinks", feature = "wecom"))]
pub use crate::sinks::{WeComWebhookConfig, WeComWebhookSink};
