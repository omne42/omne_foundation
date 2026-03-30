#[cfg(any(not(feature = "selective-sinks"), feature = "bark"))]
mod bark;
#[cfg(any(
    not(feature = "selective-sinks"),
    feature = "dingtalk",
    feature = "feishu"
))]
mod crypto;
#[cfg(any(not(feature = "selective-sinks"), feature = "dingtalk"))]
mod dingtalk;
#[cfg(any(not(feature = "selective-sinks"), feature = "discord"))]
mod discord;
#[cfg(any(not(feature = "selective-sinks"), feature = "feishu"))]
mod feishu;
#[cfg(any(not(feature = "selective-sinks"), feature = "generic-webhook"))]
mod generic_webhook;
#[cfg(any(not(feature = "selective-sinks"), feature = "github"))]
mod github;
#[cfg(any(not(feature = "selective-sinks"), feature = "feishu"))]
mod markdown;
#[cfg(any(not(feature = "selective-sinks"), feature = "pushplus"))]
mod pushplus;
#[cfg(any(not(feature = "selective-sinks"), feature = "serverchan"))]
mod serverchan;
#[cfg(any(not(feature = "selective-sinks"), feature = "slack"))]
mod slack;
#[cfg(any(not(feature = "selective-sinks"), feature = "sound"))]
mod sound;
#[cfg(any(not(feature = "selective-sinks"), feature = "telegram"))]
mod telegram;
#[cfg(any(
    not(feature = "selective-sinks"),
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
#[cfg(any(not(feature = "selective-sinks"), feature = "wecom"))]
mod wecom;

use std::future::Future;
use std::pin::Pin;

use crate::event::Event;

#[cfg(any(not(feature = "selective-sinks"), feature = "bark"))]
pub use bark::{BarkConfig, BarkSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "dingtalk"))]
pub use dingtalk::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "discord"))]
pub use discord::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "feishu"))]
pub use feishu::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "generic-webhook"))]
pub use generic_webhook::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "github"))]
pub use github::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "pushplus"))]
pub use pushplus::{PushPlusConfig, PushPlusSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "serverchan"))]
pub use serverchan::{ServerChanConfig, ServerChanSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "slack"))]
pub use slack::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "sound"))]
pub use sound::{SoundConfig, SoundSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "telegram"))]
pub use telegram::{TelegramBotConfig, TelegramBotSink};
#[cfg(any(not(feature = "selective-sinks"), feature = "wecom"))]
pub use wecom::{WeComWebhookConfig, WeComWebhookSink};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>>;
}
