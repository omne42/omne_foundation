//! Built-in sink implementations.
//!
//! This namespace groups provider-specific adapters so callers that only need the
//! core notification boundary can stay on [`crate::core`].

#[cfg(any(not(feature = "selective-sinks"), feature = "bark"))]
pub use crate::sinks::{BarkConfig, BarkSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "dingtalk"))]
pub use crate::sinks::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "discord"))]
pub use crate::sinks::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "feishu"))]
pub use crate::sinks::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "generic-webhook"))]
pub use crate::sinks::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "github"))]
pub use crate::sinks::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "pushplus"))]
pub use crate::sinks::{PushPlusConfig, PushPlusSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "serverchan"))]
pub use crate::sinks::{ServerChanConfig, ServerChanSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "slack"))]
pub use crate::sinks::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "sound"))]
pub use crate::sinks::{SoundConfig, SoundSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "telegram"))]
pub use crate::sinks::{TelegramBotConfig, TelegramBotSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "wecom"))]
pub use crate::sinks::{WeComWebhookConfig, WeComWebhookSink};
