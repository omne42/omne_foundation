mod bark;
mod crypto;
mod dingtalk;
mod discord;
mod feishu;
mod generic_webhook;
mod github;
mod http;
mod markdown;
mod pushplus;
mod serverchan;
mod slack;
mod sound;
mod telegram;
mod text;
mod wecom;

use std::future::Future;
use std::pin::Pin;

use crate::event::Event;

pub use bark::{BarkConfig, BarkSink};
pub use dingtalk::{DingTalkWebhookConfig, DingTalkWebhookSink};
pub use discord::{DiscordWebhookConfig, DiscordWebhookSink};
pub use feishu::{FeishuWebhookConfig, FeishuWebhookSink};
pub use generic_webhook::{GenericWebhookConfig, GenericWebhookSink};
pub use github::{GitHubCommentConfig, GitHubCommentSink};
pub use pushplus::{PushPlusConfig, PushPlusSink};
pub use serverchan::{ServerChanConfig, ServerChanSink};
pub use slack::{SlackWebhookConfig, SlackWebhookSink};
pub use sound::{SoundConfig, SoundSink};
pub use telegram::{TelegramBotConfig, TelegramBotSink};
pub use wecom::{WeComWebhookConfig, WeComWebhookSink};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>>;
}
