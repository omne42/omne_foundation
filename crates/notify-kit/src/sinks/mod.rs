#[cfg(any(feature = "all-sinks", feature = "bark"))]
mod bark;
#[cfg(any(feature = "all-sinks", feature = "dingtalk", feature = "feishu"))]
mod crypto;
#[cfg(any(feature = "all-sinks", feature = "dingtalk"))]
mod dingtalk;
#[cfg(any(feature = "all-sinks", feature = "discord"))]
mod discord;
#[cfg(any(feature = "all-sinks", feature = "feishu"))]
mod feishu;
#[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
mod generic_webhook;
#[cfg(any(feature = "all-sinks", feature = "github"))]
mod github;
#[cfg(any(feature = "all-sinks", feature = "feishu"))]
mod markdown;
#[cfg(any(feature = "all-sinks", feature = "pushplus"))]
mod pushplus;
#[cfg(any(feature = "all-sinks", feature = "serverchan"))]
mod serverchan;
#[cfg(any(feature = "all-sinks", feature = "slack"))]
mod slack;
#[cfg(any(feature = "all-sinks", feature = "sound"))]
mod sound;
#[cfg(any(feature = "all-sinks", feature = "telegram"))]
mod telegram;
#[cfg(any(
    feature = "all-sinks",
    feature = "bark",
    feature = "dingtalk",
    feature = "discord",
    feature = "feishu",
    feature = "generic-webhook",
    feature = "github",
    feature = "pushplus",
    feature = "serverchan",
    feature = "slack",
    feature = "telegram",
    feature = "wecom"
))]
mod text;
#[cfg(any(
    feature = "all-sinks",
    feature = "dingtalk",
    feature = "discord",
    feature = "generic-webhook",
    feature = "slack",
    feature = "wecom"
))]
mod webhook_common;
#[cfg(any(feature = "all-sinks", feature = "wecom"))]
mod wecom;

use std::future::Future;
use std::pin::Pin;

use crate::event::Event;

#[cfg(any(feature = "all-sinks", feature = "bark"))]
pub use bark::{BarkConfig, BarkSink};
#[cfg(any(feature = "all-sinks", feature = "dingtalk"))]
pub use dingtalk::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "discord"))]
pub use discord::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "feishu"))]
pub use feishu::{FeishuWebhookConfig, FeishuWebhookMediaConfig, FeishuWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "generic-webhook"))]
pub use generic_webhook::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "github"))]
pub use github::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(any(feature = "all-sinks", feature = "pushplus"))]
pub use pushplus::{PushPlusConfig, PushPlusSink};
#[cfg(any(feature = "all-sinks", feature = "serverchan"))]
pub use serverchan::{ServerChanConfig, ServerChanSink};
#[cfg(any(feature = "all-sinks", feature = "slack"))]
pub use slack::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(feature = "all-sinks", feature = "sound"))]
pub use sound::{SoundConfig, SoundSink};
#[cfg(any(feature = "all-sinks", feature = "telegram"))]
pub use telegram::{TelegramBotConfig, TelegramBotSink};
#[cfg(any(feature = "all-sinks", feature = "wecom"))]
pub use wecom::{WeComWebhookConfig, WeComWebhookSink};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>>;
}
