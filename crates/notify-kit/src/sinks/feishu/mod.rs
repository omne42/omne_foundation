mod media;
mod payload;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use self::media::{
    AccessTokenCache, FeishuAppCredentials, normalize_app_credentials, normalize_secret,
};
use crate::Event;
use crate::sinks::crypto::hmac_sha256_base64;
use crate::sinks::{BoxFuture, Sink};
use http_kit::{
    build_http_client, parse_and_validate_https_url, read_json_body_after_http_success, redact_url,
    redact_url_str, select_http_client, send_reqwest, validate_url_path_prefix,
};

const FEISHU_MAX_CHARS: usize = 4000;
const FEISHU_DEFAULT_IMAGE_UPLOAD_MAX_BYTES: usize = 10 * 1024 * 1024;

#[non_exhaustive]
#[derive(Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
    pub enable_markdown_rich_text: bool,
    pub allow_local_image_files: bool,
    pub image_upload_max_bytes: usize,
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
}

impl std::fmt::Debug for FeishuWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field("enable_markdown_rich_text", &self.enable_markdown_rich_text)
            .field("allow_local_image_files", &self.allow_local_image_files)
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
            allow_local_image_files: false,
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
    pub fn with_local_image_files(mut self, allow: bool) -> Self {
        self.allow_local_image_files = allow;
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
        app_secret: impl Into<String>,
    ) -> Self {
        self.app_id = Some(app_id.into());
        self.app_secret = Some(app_secret.into());
        self
    }
}

pub struct FeishuWebhookSink {
    webhook_url: reqwest::Url,
    client: reqwest::Client,
    timeout: Duration,
    secret: Option<String>,
    max_chars: usize,
    enforce_public_ip: bool,
    enable_markdown_rich_text: bool,
    allow_local_image_files: bool,
    image_upload_max_bytes: usize,
    app_credentials: Option<FeishuAppCredentials>,
    tenant_access_token: Arc<tokio::sync::Mutex<TenantAccessTokenState>>,
}

#[derive(Debug)]
enum TenantAccessTokenState {
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
            .field("allow_local_image_files", &self.allow_local_image_files)
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
        Self::new_internal(config, None, false)
    }

    pub fn new_strict(config: FeishuWebhookConfig) -> crate::Result<Self> {
        Self::new_internal(config, None, true)
    }

    pub async fn new_strict_async(config: FeishuWebhookConfig) -> crate::Result<Self> {
        Self::new_internal_async(config, None, true).await
    }

    pub fn new_with_secret(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal(config, Some(secret), false)
    }

    pub fn new_with_secret_strict(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal(config, Some(secret), true)
    }

    pub async fn new_with_secret_strict_async(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal_async(config, Some(secret), true).await
    }

    fn new_internal(
        config: FeishuWebhookConfig,
        secret: Option<String>,
        validate_public_ip_at_construction: bool,
    ) -> crate::Result<Self> {
        let enforce_public_ip = config.enforce_public_ip;
        if validate_public_ip_at_construction && !enforce_public_ip {
            return Err(anyhow::anyhow!("feishu strict mode requires public ip check").into());
        }

        let app_credentials = normalize_app_credentials(config.app_id, config.app_secret)?;
        let webhook_url = parse_and_validate_https_url(
            &config.webhook_url,
            &["open.feishu.cn", "open.larksuite.com"],
        )?;
        validate_url_path_prefix(&webhook_url, "/open-apis/bot/v2/hook/")?;
        let client = build_http_client(config.timeout)?;
        if validate_public_ip_at_construction {
            if tokio::runtime::Handle::try_current().is_ok() {
                return Err(anyhow::anyhow!(
                    "feishu strict constructor cannot run inside tokio runtime; use new_strict_async/new_with_secret_strict_async"
                )
                .into());
            }
            Self::validate_public_ip_at_construction_sync(&client, config.timeout, &webhook_url)?;
        }

        Ok(Self {
            webhook_url,
            client,
            timeout: config.timeout,
            secret,
            max_chars: config.max_chars,
            enforce_public_ip,
            enable_markdown_rich_text: config.enable_markdown_rich_text,
            allow_local_image_files: config.allow_local_image_files,
            image_upload_max_bytes: config.image_upload_max_bytes,
            app_credentials,
            tenant_access_token: Arc::new(tokio::sync::Mutex::new(TenantAccessTokenState::Empty)),
        })
    }

    async fn new_internal_async(
        config: FeishuWebhookConfig,
        secret: Option<String>,
        validate_public_ip_at_construction: bool,
    ) -> crate::Result<Self> {
        let enforce_public_ip = config.enforce_public_ip;
        if validate_public_ip_at_construction && !enforce_public_ip {
            return Err(anyhow::anyhow!("feishu strict mode requires public ip check").into());
        }

        let app_credentials = normalize_app_credentials(config.app_id, config.app_secret)?;
        let webhook_url = parse_and_validate_https_url(
            &config.webhook_url,
            &["open.feishu.cn", "open.larksuite.com"],
        )?;
        validate_url_path_prefix(&webhook_url, "/open-apis/bot/v2/hook/")?;
        let client = build_http_client(config.timeout)?;
        if validate_public_ip_at_construction {
            select_http_client(&client, config.timeout, &webhook_url, true)
                .await
                .map(|_| ())?;
        }

        Ok(Self {
            webhook_url,
            client,
            timeout: config.timeout,
            secret,
            max_chars: config.max_chars,
            enforce_public_ip,
            enable_markdown_rich_text: config.enable_markdown_rich_text,
            allow_local_image_files: config.allow_local_image_files,
            image_upload_max_bytes: config.image_upload_max_bytes,
            app_credentials,
            tenant_access_token: Arc::new(tokio::sync::Mutex::new(TenantAccessTokenState::Empty)),
        })
    }

    fn ensure_success_response(body: &serde_json::Value) -> crate::Result<()> {
        let Some(code) = body["StatusCode"]
            .as_i64()
            .or_else(|| body["code"].as_i64())
        else {
            return Err(anyhow::anyhow!(
                "feishu api error: missing status code (response body omitted)"
            )
            .into());
        };

        if code == 0 {
            return Ok(());
        }

        Err(anyhow::anyhow!("feishu api error: code={code} (response body omitted)").into())
    }

    fn validate_public_ip_at_construction_sync(
        client: &reqwest::Client,
        timeout: Duration,
        webhook_url: &reqwest::Url,
    ) -> crate::Result<()> {
        let client = client.clone();
        let webhook_url = webhook_url.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow::anyhow!("build tokio runtime: {err}"))?;
        Ok(rt.block_on(async move {
            select_http_client(&client, timeout, &webhook_url, true)
                .await
                .map(|_| ())
        })?)
    }
}

impl Sink for FeishuWebhookSink {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = select_http_client(
                &self.client,
                self.timeout,
                &self.webhook_url,
                self.enforce_public_ip,
            )
            .await?;
            let (timestamp, sign) = if let Some(secret) = self.secret.as_deref() {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|err| anyhow::anyhow!("get unix timestamp: {err}"))?
                    .as_secs()
                    .to_string();

                let string_to_sign = format!("{timestamp}\n{secret}");
                let sign = hmac_sha256_base64(secret, &string_to_sign)?;

                (Some(timestamp), Some(sign))
            } else {
                (None, None)
            };

            let payload = self
                .build_payload(event, timestamp.as_deref(), sign.as_deref())
                .await?;

            let resp = send_reqwest(
                client.post(self.webhook_url.as_str()).json(&payload),
                "feishu webhook",
            )
            .await?;

            let body = read_json_body_after_http_success(resp, "feishu webhook").await?;
            Self::ensure_success_response(&body)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    #[test]
    fn builds_expected_text_payload() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = FeishuWebhookSink::build_text_payload(&event, FEISHU_MAX_CHARS, None, None);
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "text");
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn builds_post_payload_for_markdown_body() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("hello [lark](https://open.feishu.cn)\n\n![img](https://example.com/a.png)")
            .with_tag("thread_id", "t1");

        let sink = FeishuWebhookSink::new(FeishuWebhookConfig::new(
            "https://open.feishu.cn/open-apis/bot/v2/hook/x",
        ))
        .expect("build sink");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let payload = rt
            .block_on(sink.build_payload(&event, None, None))
            .expect("build payload");
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "post");

        let content = payload["content"]["post"]["zh_cn"]["content"]
            .as_array()
            .expect("array content");
        assert!(!content.is_empty());

        let text_payload = payload.to_string();
        assert!(text_payload.contains("\"tag\":\"a\""), "{text_payload}");
        assert!(text_payload.contains("thread_id=t1"), "{text_payload}");
        assert!(text_payload.contains("[image:img]"), "{text_payload}");
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = FeishuWebhookConfig::new("http://open.feishu.cn/open-apis/bot/v2/hook/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = FeishuWebhookConfig::new("https://example.com/open-apis/bot/v2/hook/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_path() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/api/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn strict_requires_public_ip_check() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_public_ip_check(false);
        let err = FeishuWebhookSink::new_strict(cfg).expect_err("expected strict validation");
        assert!(err.to_string().contains("public ip"), "{err:#}");
    }

    #[test]
    fn strict_sync_constructor_rejects_inside_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x");
            let err =
                FeishuWebhookSink::new_strict(cfg).expect_err("expected runtime constructor error");
            assert!(err.to_string().contains("new_strict_async"), "{err:#}");
        });
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let url = "https://open.feishu.cn/open-apis/bot/v2/hook/secret_token";
        let cfg = FeishuWebhookConfig::new(url);
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret_token"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("open.feishu.cn"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = FeishuWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret_token"), "{sink_dbg}");
        assert!(sink_dbg.contains("open.feishu.cn"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn builds_payload_with_signature_fields() {
        let event = Event::new("kind", crate::Severity::Info, "title");
        let payload = FeishuWebhookSink::build_text_payload(
            &event,
            FEISHU_MAX_CHARS,
            Some("123"),
            Some("sig"),
        );
        assert_eq!(payload["timestamp"].as_str().unwrap_or(""), "123");
        assert_eq!(payload["sign"].as_str().unwrap_or(""), "sig");
    }

    #[test]
    fn trims_secret() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x");
        let sink =
            FeishuWebhookSink::new_with_secret(cfg, "  my_secret  ").expect("build secret sink");
        assert_eq!(sink.secret.as_deref(), Some("my_secret"));
    }

    #[test]
    fn payload_respects_max_chars() {
        let event = Event::new("kind", crate::Severity::Info, "title").with_body("x".repeat(100));
        let payload = FeishuWebhookSink::build_text_payload(&event, 10, None, None);
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.chars().count() <= 10, "{text}");
        assert!(text.ends_with("..."), "{text}");
    }

    #[test]
    fn normalizes_app_credentials() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_app_credentials("  app_id  ", "  app_secret  ");
        let sink = FeishuWebhookSink::new(cfg).expect("build sink");
        let creds = sink.app_credentials.expect("credentials");
        assert_eq!(creds.app_id, "app_id");
        assert_eq!(creds.app_secret, "app_secret");
    }

    #[test]
    fn local_image_files_are_disabled_by_default() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");

            let err = sink
                .load_image("./should-not-be-read.png")
                .await
                .expect_err("local files should be disabled");
            assert!(
                err.to_string().contains("local image files are disabled"),
                "{err:#}"
            );
        });
    }

    #[test]
    fn local_image_files_require_explicit_opt_in() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "notify-kit-feishu-local-image-{}-{unique}.png",
                std::process::id()
            ));
            std::fs::write(&path, b"png").expect("write local image");

            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_local_image_files(true),
            )
            .expect("build sink");

            let loaded = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect("load local image");
            assert_eq!(loaded.bytes, b"png");
            assert_eq!(loaded.content_type, "image/png");

            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn local_image_files_reject_non_regular_paths() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "notify-kit-feishu-local-image-dir-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create local image dir");

            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_local_image_files(true),
            )
            .expect("build sink");

            let err = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect_err("directories should be rejected");
            assert!(err.to_string().contains("regular file"), "{err:#}");

            let _ = std::fs::remove_dir_all(path);
        });
    }

    #[test]
    fn local_image_files_reject_oversized_files_before_upload() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "notify-kit-feishu-local-image-large-{}-{unique}.png",
                std::process::id()
            ));
            std::fs::write(&path, vec![b'x'; 5]).expect("write oversized image");

            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_local_image_files(true)
                    .with_image_upload_max_bytes(4),
            )
            .expect("build sink");

            let err = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect_err("oversized files should be rejected");
            assert!(err.to_string().contains("too large"), "{err:#}");

            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn concurrent_token_refresh_is_singleflight() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&hits);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            server_hits.fetch_add(1, Ordering::SeqCst);

            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf).expect("read request");

            let body = r#"{"code":0,"tenant_access_token":"token","expires_in":7200}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let mut sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");
            sink.webhook_url =
                reqwest::Url::parse(&format!("http://{addr}/open-apis/bot/v2/hook/x"))
                    .expect("parse local webhook url");
            sink.enforce_public_ip = false;

            let sink = Arc::new(sink);
            let mut tasks = Vec::new();
            for _ in 0..8 {
                let sink = Arc::clone(&sink);
                tasks.push(tokio::spawn(async move {
                    sink.ensure_tenant_access_token().await
                }));
            }

            for task in tasks {
                let token = task
                    .await
                    .expect("join token task")
                    .expect("resolve tenant token");
                assert_eq!(token, "token");
            }
        });

        server.join().expect("join server");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_token_refresh_resets_state_and_allows_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let (first_hit_tx, first_hit_rx) = tokio::sync::oneshot::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut first_stream, _) = listener.accept().expect("accept first connection");
            let mut buf = [0_u8; 1024];
            let _ = first_stream.read(&mut buf).expect("read first request");
            first_hit_tx.send(()).expect("signal first request");
            release_first_rx.recv().expect("release first request");
            drop(first_stream);

            let (mut second_stream, _) = listener.accept().expect("accept second connection");
            let _ = second_stream.read(&mut buf).expect("read second request");
            let body = r#"{"code":0,"tenant_access_token":"token-after-retry","expires_in":7200}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            second_stream
                .write_all(response.as_bytes())
                .expect("write second response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let mut sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");
            sink.webhook_url =
                reqwest::Url::parse(&format!("http://{addr}/open-apis/bot/v2/hook/x"))
                    .expect("parse local webhook url");
            sink.enforce_public_ip = false;

            let sink = Arc::new(sink);
            let refresh_task = tokio::spawn({
                let sink = Arc::clone(&sink);
                async move { sink.ensure_tenant_access_token().await }
            });

            first_hit_rx.await.expect("wait for first token request");
            refresh_task.abort();
            let _ = refresh_task.await;
            release_first_tx.send(()).expect("release first request");

            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let guard = sink.tenant_access_token.lock().await;
                    if matches!(&*guard, TenantAccessTokenState::Empty) {
                        break;
                    }
                    drop(guard);
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("refresh cancellation should reset token state");

            let token = sink
                .ensure_tenant_access_token()
                .await
                .expect("retry should fetch a fresh token");
            assert_eq!(token, "token-after-retry");
        });

        server.join().expect("join server");
    }

    #[test]
    fn response_requires_explicit_success_code() {
        let body = serde_json::json!({});
        let err =
            FeishuWebhookSink::ensure_success_response(&body).expect_err("expected missing code");
        assert!(err.to_string().contains("missing status code"), "{err:#}");
    }

    #[test]
    fn response_accepts_zero_code() {
        let body = serde_json::json!({ "StatusCode": 0 });
        FeishuWebhookSink::ensure_success_response(&body).expect("expected success");

        let body = serde_json::json!({ "code": 0 });
        FeishuWebhookSink::ensure_success_response(&body).expect("expected success");
    }
}
