#![forbid(unsafe_code)]

//! `notify-kit` keeps the core `Event` / `Hub` / `Sink` surface always available, while built-in
//! sink implementations are feature-gated so downstream workspaces can trim transport-specific
//! code and dependencies.
//!
//! Recommended layering:
//! - [`crate::core`]: provider-agnostic foundation surface
//! - [`crate::providers`]: built-in transport integrations
//! - [`crate::env`]: optional bootstrap helper for a small shared env convention
//!
//! The crate root keeps compatibility re-exports for existing callers, but new code should prefer
//! the namespaced modules so the core/provider boundary stays explicit.

pub mod core;

/// Convenience env wiring helpers.
///
/// This module stays separate from the crate root Hub/Sink surface on purpose: `notify-kit` does
/// not define a mandatory env protocol, and projects that need product-specific configuration
/// should keep that wiring in their own integration layer.
#[cfg(feature = "env-standard")]
pub mod env;
mod error;
mod event;
mod hub;
mod log;
pub mod providers;
mod sinks;

pub use crate::error::{Error, ErrorKind, SinkFailure};
pub type Result<T> = std::result::Result<T, Error>;

pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, HubLimits, TryNotifyError};
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
#[cfg(feature = "sink-github-comment")]
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

#[cfg(any(
    feature = "sink-bark",
    feature = "sink-dingtalk",
    feature = "sink-feishu",
    feature = "sink-github-comment",
    feature = "sink-pushplus",
    feature = "sink-serverchan",
    feature = "sink-telegram",
))]
pub(crate) use secret_kit::SecretString;
