//! Built-in provider-specific notification integrations.
//!
//! Each submodule is feature-gated so downstream workspaces can depend on the narrow core API
//! while opting into only the transport surfaces they actually need.

#[cfg(feature = "sink-bark")]
pub mod bark {
    pub use crate::sinks::{BarkConfig, BarkSink};
}

#[cfg(feature = "sink-dingtalk")]
pub mod dingtalk {
    pub use crate::sinks::{DingTalkWebhookConfig, DingTalkWebhookSink};
}

#[cfg(feature = "sink-discord")]
pub mod discord {
    pub use crate::sinks::{DiscordWebhookConfig, DiscordWebhookSink};
}

#[cfg(feature = "sink-feishu")]
pub mod feishu {
    pub use crate::sinks::{FeishuWebhookConfig, FeishuWebhookSink};
}

#[cfg(feature = "sink-generic-webhook")]
pub mod generic_webhook {
    pub use crate::sinks::{GenericWebhookConfig, GenericWebhookSink};
}

#[cfg(feature = "sink-github-comment")]
pub mod github_comment {
    pub use crate::sinks::{GitHubCommentConfig, GitHubCommentSink};
}

#[cfg(feature = "sink-pushplus")]
pub mod pushplus {
    pub use crate::sinks::{PushPlusConfig, PushPlusSink};
}

#[cfg(feature = "sink-serverchan")]
pub mod serverchan {
    pub use crate::sinks::{ServerChanConfig, ServerChanSink};
}

#[cfg(feature = "sink-slack")]
pub mod slack {
    pub use crate::sinks::{SlackWebhookConfig, SlackWebhookSink};
}

#[cfg(feature = "sink-sound")]
pub mod sound {
    pub use crate::sinks::{SoundConfig, SoundSink};
}

#[cfg(feature = "sink-telegram")]
pub mod telegram {
    pub use crate::sinks::{TelegramBotConfig, TelegramBotSink};
}

#[cfg(feature = "sink-wecom")]
pub mod wecom {
    pub use crate::sinks::{WeComWebhookConfig, WeComWebhookSink};
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "sink-feishu")]
    #[test]
    fn feishu_provider_module_reexports_sink_types() {
        let _ = crate::providers::feishu::FeishuWebhookConfig::new(
            "https://open.feishu.cn/open-apis/bot/v2/hook/example",
        );
    }

    #[cfg(feature = "sink-sound")]
    #[test]
    fn sound_provider_module_reexports_sink_types() {
        let _ = crate::providers::sound::SoundConfig { command_argv: None };
    }
}
