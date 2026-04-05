use std::path::PathBuf;
use std::sync::Arc;

use crate::SecretString;
use http_kit::{
    HttpClientOptions, HttpClientProfile, build_http_client_profile, parse_and_validate_https_url,
    redact_url, validate_url_path_prefix,
};

use super::FeishuWebhookConfig;
use super::media::{
    AccessTokenCache, FeishuAppCredentials, normalize_app_credentials,
    normalize_local_image_base_dir, normalize_local_image_roots, normalize_secret,
};

pub struct FeishuWebhookSink {
    pub(super) webhook_url: reqwest::Url,
    pub(super) http: HttpClientProfile,
    pub(super) secret: Option<SecretString>,
    pub(super) max_chars: usize,
    pub(super) enforce_public_ip: bool,
    pub(super) enable_markdown_rich_text: bool,
    pub(super) allow_remote_image_urls: bool,
    pub(super) allow_local_image_files: bool,
    pub(super) local_image_roots: Vec<PathBuf>,
    pub(super) local_image_base_dir: Option<PathBuf>,
    pub(super) image_upload_max_bytes: usize,
    pub(super) app_credentials: Option<FeishuAppCredentials>,
    pub(super) tenant_access_token: Arc<tokio::sync::Mutex<TenantAccessTokenState>>,
}

#[derive(Debug)]
pub(super) enum TenantAccessTokenState {
    Empty,
    Ready(AccessTokenCache),
    Refreshing(Arc<tokio::sync::Notify>),
}

impl std::fmt::Debug for FeishuWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookSink")
            .field("webhook_url", &redact_url(&self.webhook_url))
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field("enable_markdown_rich_text", &self.enable_markdown_rich_text)
            .field("allow_remote_image_urls", &self.allow_remote_image_urls)
            .field("allow_local_image_files", &self.allow_local_image_files)
            .field("local_image_roots", &self.local_image_roots)
            .field("local_image_base_dir", &self.local_image_base_dir)
            .field("image_upload_max_bytes", &self.image_upload_max_bytes)
            .field(
                "app_credentials",
                &self.app_credentials.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

impl FeishuWebhookSink {
    pub fn new(config: FeishuWebhookConfig) -> crate::Result<Self> {
        Self::new_internal(config, None)
    }

    pub fn new_strict(config: FeishuWebhookConfig) -> crate::Result<Self> {
        let sink = Self::new_internal(config, None)?;
        sink.validate_public_ip_sync()?;
        Ok(sink)
    }

    pub async fn new_strict_async(config: FeishuWebhookConfig) -> crate::Result<Self> {
        let sink = Self::new_internal(config, None)?;
        sink.validate_public_ip().await?;
        Ok(sink)
    }

    pub fn new_with_secret(
        config: FeishuWebhookConfig,
        secret: impl Into<SecretString>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal(config, Some(secret))
    }

    pub fn new_with_secret_strict(
        config: FeishuWebhookConfig,
        secret: impl Into<SecretString>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        let sink = Self::new_internal(config, Some(secret))?;
        sink.validate_public_ip_sync()?;
        Ok(sink)
    }

    pub async fn new_with_secret_strict_async(
        config: FeishuWebhookConfig,
        secret: impl Into<SecretString>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        let sink = Self::new_internal(config, Some(secret))?;
        sink.validate_public_ip().await?;
        Ok(sink)
    }

    fn new_internal(
        config: FeishuWebhookConfig,
        secret: Option<SecretString>,
    ) -> crate::Result<Self> {
        let enforce_public_ip = config.enforce_public_ip;

        let app_credentials = normalize_app_credentials(config.app_id, config.app_secret)?;
        let local_image_roots =
            normalize_local_image_roots(config.allow_local_image_files, config.local_image_roots)?;
        let local_image_base_dir = normalize_local_image_base_dir(
            config.allow_local_image_files,
            config.local_image_base_dir,
        )?;
        let webhook_url = parse_and_validate_https_url(
            &config.webhook_url,
            &["open.feishu.cn", "open.larksuite.com"],
        )?;
        validate_url_path_prefix(&webhook_url, "/open-apis/bot/v2/hook/")?;
        let http = build_http_client_profile(&HttpClientOptions {
            timeout: Some(config.timeout),
            ..Default::default()
        })?;

        Ok(Self {
            webhook_url,
            http,
            secret,
            max_chars: config.max_chars,
            enforce_public_ip,
            enable_markdown_rich_text: config.enable_markdown_rich_text,
            allow_remote_image_urls: config.allow_remote_image_urls,
            allow_local_image_files: config.allow_local_image_files,
            local_image_roots,
            local_image_base_dir,
            image_upload_max_bytes: config.image_upload_max_bytes,
            app_credentials,
            tenant_access_token: Arc::new(tokio::sync::Mutex::new(TenantAccessTokenState::Empty)),
        })
    }

    pub async fn validate_public_ip(&self) -> crate::Result<()> {
        self.ensure_public_ip_validation_enabled("validate_public_ip")?;
        self.http
            .select_for_url(&self.webhook_url, true)
            .await
            .map(|_| ())
            .map_err(crate::Error::from)
    }

    pub fn validate_public_ip_sync(&self) -> crate::Result<()> {
        self.ensure_public_ip_validation_enabled("validate_public_ip_sync")?;
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(crate::error::tagged_message(
                crate::ErrorKind::Config,
                "feishu public-ip validation cannot block inside a tokio runtime; use validate_public_ip()/new_strict_async/new_with_secret_strict_async",
            )
            .into());
        }
        Self::validate_public_ip_sync_with_profile(&self.http, &self.webhook_url)
    }

    fn ensure_public_ip_validation_enabled(&self, operation: &str) -> crate::Result<()> {
        if self.enforce_public_ip {
            return Ok(());
        }
        Err(crate::error::tagged_message(
            crate::ErrorKind::Config,
            format!(
                "{operation} requires FeishuWebhookConfig::with_public_ip_check(true) so public ip validation stays enabled"
            ),
        )
        .into())
    }

    fn validate_public_ip_sync_with_profile(
        http: &HttpClientProfile,
        webhook_url: &reqwest::Url,
    ) -> crate::Result<()> {
        let http = http.clone();
        let webhook_url = webhook_url.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow::anyhow!("build tokio runtime: {err}"))?;
        Ok(rt.block_on(async move { http.select_for_url(&webhook_url, true).await.map(|_| ()) })?)
    }
}
