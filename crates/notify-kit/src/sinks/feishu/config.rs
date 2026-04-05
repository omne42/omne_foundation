use std::path::PathBuf;
use std::time::Duration;

use crate::SecretString;
use http_kit::redact_url_str;

use super::{FEISHU_DEFAULT_IMAGE_UPLOAD_MAX_BYTES, FEISHU_MAX_CHARS};

#[non_exhaustive]
#[derive(Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
    pub enable_markdown_rich_text: bool,
    pub allow_remote_image_urls: bool,
    pub allow_local_image_files: bool,
    pub local_image_roots: Vec<PathBuf>,
    pub local_image_base_dir: Option<PathBuf>,
    pub image_upload_max_bytes: usize,
    pub app_id: Option<String>,
    pub app_secret: Option<SecretString>,
}

impl std::fmt::Debug for FeishuWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field("enable_markdown_rich_text", &self.enable_markdown_rich_text)
            .field("allow_remote_image_urls", &self.allow_remote_image_urls)
            .field("allow_local_image_files", &self.allow_local_image_files)
            .field("local_image_roots", &self.local_image_roots)
            .field("local_image_base_dir", &self.local_image_base_dir)
            .field("image_upload_max_bytes", &self.image_upload_max_bytes)
            .field("app_id", &self.app_id.as_ref().map(|_| "<redacted>"))
            .field(
                "app_secret",
                &self.app_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl FeishuWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            timeout: Duration::from_secs(2),
            max_chars: FEISHU_MAX_CHARS,
            enforce_public_ip: true,
            enable_markdown_rich_text: true,
            allow_remote_image_urls: false,
            allow_local_image_files: false,
            local_image_roots: Vec::new(),
            local_image_base_dir: None,
            image_upload_max_bytes: FEISHU_DEFAULT_IMAGE_UPLOAD_MAX_BYTES,
            app_id: None,
            app_secret: None,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    #[must_use]
    pub fn with_public_ip_check(mut self, enforce_public_ip: bool) -> Self {
        self.enforce_public_ip = enforce_public_ip;
        self
    }

    #[must_use]
    pub fn with_markdown_rich_text(mut self, enable: bool) -> Self {
        self.enable_markdown_rich_text = enable;
        self
    }

    #[must_use]
    pub fn with_remote_image_urls(mut self, allow: bool) -> Self {
        self.allow_remote_image_urls = allow;
        self
    }

    #[must_use]
    pub fn with_local_image_files(mut self, allow: bool) -> Self {
        self.allow_local_image_files = allow;
        self
    }

    #[must_use]
    pub fn with_local_image_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.local_image_roots.push(root.into());
        self
    }

    #[must_use]
    pub fn with_local_image_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.local_image_roots = roots.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn with_local_image_base_dir(mut self, base_dir: impl Into<PathBuf>) -> Self {
        self.local_image_base_dir = Some(base_dir.into());
        self
    }

    #[must_use]
    pub fn with_image_upload_max_bytes(mut self, max_bytes: usize) -> Self {
        self.image_upload_max_bytes = max_bytes;
        self
    }

    #[must_use]
    pub fn with_app_credentials(
        mut self,
        app_id: impl Into<String>,
        app_secret: impl Into<SecretString>,
    ) -> Self {
        self.app_id = Some(app_id.into());
        self.app_secret = Some(app_secret.into());
        self
    }
}
