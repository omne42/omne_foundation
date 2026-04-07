#[cfg(feature = "sink-bark")]
mod bark;
#[cfg(any(feature = "sink-dingtalk", feature = "sink-feishu"))]
mod crypto;
#[cfg(feature = "sink-dingtalk")]
mod dingtalk;
#[cfg(feature = "sink-discord")]
mod discord;
#[cfg(feature = "sink-feishu")]
mod feishu;
#[cfg(feature = "sink-generic-webhook")]
mod generic_webhook;
#[cfg(feature = "sink-github-comment")]
mod github;
#[cfg(feature = "sink-feishu")]
mod markdown;
#[cfg(feature = "sink-pushplus")]
mod pushplus;
#[cfg(feature = "sink-serverchan")]
mod serverchan;
#[cfg(feature = "sink-slack")]
mod slack;
#[cfg(feature = "sink-sound")]
mod sound;
#[cfg(feature = "sink-telegram")]
mod telegram;
#[cfg(any(
    feature = "sink-bark",
    feature = "sink-dingtalk",
    feature = "sink-discord",
    feature = "sink-feishu",
    feature = "sink-generic-webhook",
    feature = "sink-github-comment",
    feature = "sink-pushplus",
    feature = "sink-serverchan",
    feature = "sink-slack",
    feature = "sink-telegram",
    feature = "sink-wecom",
))]
mod text;
#[cfg(any(
    feature = "sink-bark",
    feature = "sink-dingtalk",
    feature = "sink-discord",
    feature = "sink-feishu",
    feature = "sink-generic-webhook",
    feature = "sink-github-comment",
    feature = "sink-pushplus",
    feature = "sink-serverchan",
    feature = "sink-slack",
    feature = "sink-telegram",
    feature = "sink-wecom",
))]
mod webhook_transport;
#[cfg(feature = "sink-wecom")]
mod wecom;

use std::future::Future;
use std::pin::Pin;

use crate::event::Event;

#[cfg(feature = "sink-bark")]
pub use bark::{BarkConfig, BarkSink};
#[cfg(feature = "sink-dingtalk")]
pub use dingtalk::{DingTalkWebhookConfig, DingTalkWebhookSink};
#[cfg(feature = "sink-discord")]
pub use discord::{DiscordWebhookConfig, DiscordWebhookSink};
#[cfg(feature = "sink-feishu")]
pub use feishu::{FeishuWebhookConfig, FeishuWebhookSink};
#[cfg(feature = "sink-generic-webhook")]
pub use generic_webhook::{GenericWebhookConfig, GenericWebhookSink};
#[cfg(feature = "sink-github-comment")]
pub use github::{GitHubCommentConfig, GitHubCommentSink};
#[cfg(feature = "sink-pushplus")]
pub use pushplus::{PushPlusConfig, PushPlusSink};
#[cfg(feature = "sink-serverchan")]
pub use serverchan::{ServerChanConfig, ServerChanSink};
#[cfg(feature = "sink-slack")]
pub use slack::{SlackWebhookConfig, SlackWebhookSink};
#[cfg(feature = "sink-sound")]
pub use sound::{SoundConfig, SoundSink};
#[cfg(feature = "sink-telegram")]
pub use telegram::{TelegramBotConfig, TelegramBotSink};
#[cfg(feature = "sink-wecom")]
pub use wecom::{WeComWebhookConfig, WeComWebhookSink};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>>;
}
