use std::io::Read as _;
use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use crate::log::{warn_feishu_image_load_failed, warn_feishu_image_upload_failed};
use crate::sinks::BoxFuture;
use http_kit::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, http_status_text_error, parse_and_validate_https_url_basic,
    read_json_body_limited, read_text_body_limited, response_body_read_error, select_http_client,
    send_reqwest,
};

use super::FeishuWebhookSink;

#[derive(Debug, Clone)]
pub(super) struct FeishuAppCredentials {
    pub(super) app_id: String,
    pub(super) app_secret: String,
}

#[derive(Debug, Clone)]
pub(super) struct AccessTokenCache {
    pub(super) token: String,
    pub(super) expires_at: Instant,
}

#[derive(Debug)]
pub(super) struct LoadedImage {
    pub(super) bytes: Vec<u8>,
    pub(super) file_name: String,
    pub(super) content_type: String,
}

struct TenantAccessTokenRefreshGuard {
    state: std::sync::Arc<tokio::sync::Mutex<super::TenantAccessTokenState>>,
    notify: std::sync::Arc<tokio::sync::Notify>,
    armed: bool,
}

impl TenantAccessTokenRefreshGuard {
    fn new(
        state: std::sync::Arc<tokio::sync::Mutex<super::TenantAccessTokenState>>,
        notify: std::sync::Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            state,
            notify,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TenantAccessTokenRefreshGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let state = std::sync::Arc::clone(&self.state);
        let notify = std::sync::Arc::clone(&self.notify);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            drop(handle.spawn(async move {
                let mut guard = state.lock().await;
                if matches!(
                    &*guard,
                    super::TenantAccessTokenState::Refreshing(current)
                        if std::sync::Arc::ptr_eq(current, &notify)
                ) {
                    *guard = super::TenantAccessTokenState::Empty;
                }
                drop(guard);
                notify.notify_waiters();
            }));
        }
    }
}

impl FeishuWebhookSink {
    pub(super) async fn resolve_single_image_key(&self, src: &str) -> Option<String> {
        self.app_credentials.as_ref()?;

        let loaded = match self.load_image(src).await {
            Ok(loaded) => loaded,
            Err(err) => {
                warn_feishu_image_load_failed(src, &err.to_string());
                return None;
            }
        };

        match self.upload_image(loaded).await {
            Ok(image_key) => Some(image_key),
            Err(err) => {
                warn_feishu_image_upload_failed(src, &err.to_string());
                None
            }
        }
    }

    pub(super) async fn load_image(&self, src: &str) -> crate::Result<LoadedImage> {
        if src.starts_with("https://") {
            return self.load_remote_image(src).await;
        }

        if src.contains("://") {
            return Err(anyhow::anyhow!("unsupported image url scheme").into());
        }

        if !self.allow_local_image_files {
            return Err(anyhow::anyhow!("local image files are disabled").into());
        }

        let bytes = read_local_image_file(src.to_string(), self.image_upload_max_bytes).await?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("image file is empty").into());
        }
        if bytes.len() > self.image_upload_max_bytes {
            return Err(anyhow::anyhow!("image file too large for upload").into());
        }

        let path = Path::new(src);
        let file_name = path
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        let content_type = guess_image_mime(path.extension().and_then(|v| v.to_str()));

        Ok(LoadedImage {
            bytes,
            file_name,
            content_type,
        })
    }

    pub(super) async fn load_remote_image(&self, src: &str) -> crate::Result<LoadedImage> {
        let url = parse_and_validate_https_url_basic(src)?;
        let client =
            select_http_client(&self.client, self.timeout, &url, self.enforce_public_ip).await?;

        let resp = send_reqwest(client.get(url.clone()), "feishu image download").await?;
        let status = resp.status();
        if !status.is_success() {
            let body = read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES)
                .await
                .map_err(|err| {
                    response_body_read_error("feishu image download http error", status, &err)
                })?;
            return Err(http_status_text_error("feishu image download", status, &body).into());
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            .filter(|v| v.starts_with("image/"))
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                guess_image_mime(Path::new(url.path()).extension().and_then(|v| v.to_str()))
            });

        let bytes = read_bytes_body_limited(resp, self.image_upload_max_bytes).await?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("downloaded image is empty").into());
        }

        let file_name = Path::new(url.path())
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        Ok(LoadedImage {
            bytes,
            file_name,
            content_type,
        })
    }

    pub(super) async fn upload_image(&self, image: LoadedImage) -> crate::Result<String> {
        let access_token = self.ensure_tenant_access_token().await?;
        let mut upload_url = self.webhook_url.clone();
        upload_url.set_path("/open-apis/im/v1/images");
        upload_url.set_query(None);

        let client = select_http_client(
            &self.client,
            self.timeout,
            &upload_url,
            self.enforce_public_ip,
        )
        .await?;

        let part = reqwest::multipart::Part::bytes(image.bytes)
            .file_name(image.file_name)
            .mime_str(&image.content_type)
            .map_err(|err| anyhow::anyhow!("set image part mime: {err}"))?;
        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part("image", part);

        let resp = send_reqwest(
            client
                .post(upload_url)
                .bearer_auth(access_token)
                .multipart(form),
            "feishu image upload",
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES)
                .await
                .map_err(|err| {
                    response_body_read_error("feishu image upload http error", status, &err)
                })?;
            return Err(http_status_text_error("feishu image upload", status, &body).into());
        }

        let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;
        let code = body["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(anyhow::anyhow!("feishu image upload api error: code={code}").into());
        }

        let image_key = body["data"]["image_key"]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("feishu image upload api error: missing image_key"))?;

        Ok(image_key.to_string())
    }

    pub(super) async fn ensure_tenant_access_token(&self) -> crate::Result<String> {
        let Some(credentials) = self.app_credentials.as_ref().cloned() else {
            return Err(anyhow::anyhow!(
                "feishu app credentials are required for markdown image upload"
            )
            .into());
        };

        loop {
            let guard = self.tenant_access_token.lock().await;
            match &*guard {
                super::TenantAccessTokenState::Ready(cached)
                    if cached.expires_at > Instant::now() =>
                {
                    return Ok(cached.token.clone());
                }
                super::TenantAccessTokenState::Refreshing(notify) => {
                    let notify = std::sync::Arc::clone(notify);
                    drop(guard);
                    notify.notified().await;
                    continue;
                }
                super::TenantAccessTokenState::Empty | super::TenantAccessTokenState::Ready(_) => {
                    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
                    drop(guard);

                    {
                        let mut guard = self.tenant_access_token.lock().await;
                        match &*guard {
                            super::TenantAccessTokenState::Ready(cached)
                                if cached.expires_at > Instant::now() =>
                            {
                                return Ok(cached.token.clone());
                            }
                            super::TenantAccessTokenState::Refreshing(existing) => {
                                let notify = std::sync::Arc::clone(existing);
                                drop(guard);
                                notify.notified().await;
                                continue;
                            }
                            super::TenantAccessTokenState::Empty
                            | super::TenantAccessTokenState::Ready(_) => {
                                *guard = super::TenantAccessTokenState::Refreshing(
                                    std::sync::Arc::clone(&notify),
                                );
                            }
                        }
                    }

                    let mut refresh_guard = TenantAccessTokenRefreshGuard::new(
                        std::sync::Arc::clone(&self.tenant_access_token),
                        std::sync::Arc::clone(&notify),
                    );
                    let result = self.fetch_tenant_access_token(&credentials).await;
                    let mut guard = self.tenant_access_token.lock().await;
                    if matches!(
                        &*guard,
                        super::TenantAccessTokenState::Refreshing(current)
                            if std::sync::Arc::ptr_eq(current, &notify)
                    ) {
                        *guard = match &result {
                            Ok(cache) => super::TenantAccessTokenState::Ready(cache.clone()),
                            Err(_) => super::TenantAccessTokenState::Empty,
                        };
                    }
                    drop(guard);
                    refresh_guard.disarm();
                    notify.notify_waiters();
                    return result.map(|cache| cache.token);
                }
            }
        }
    }

    async fn fetch_tenant_access_token(
        &self,
        credentials: &FeishuAppCredentials,
    ) -> crate::Result<AccessTokenCache> {
        let mut token_url = self.webhook_url.clone();
        token_url.set_path("/open-apis/auth/v3/tenant_access_token/internal");
        token_url.set_query(None);

        let client = select_http_client(
            &self.client,
            self.timeout,
            &token_url,
            self.enforce_public_ip,
        )
        .await?;

        let payload = serde_json::json!({
            "app_id": credentials.app_id,
            "app_secret": credentials.app_secret,
        });

        let resp = send_reqwest(
            client.post(token_url).json(&payload),
            "feishu tenant access token",
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES)
                .await
                .map_err(|err| {
                    response_body_read_error("feishu tenant access token http error", status, &err)
                })?;
            return Err(http_status_text_error("feishu tenant access token", status, &body).into());
        }

        let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;
        let code = body["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(
                anyhow::anyhow!("feishu tenant access token api error: code={code}").into(),
            );
        }

        let token = body["tenant_access_token"]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("feishu tenant access token api error: missing token"))?
            .to_string();

        let expires_in = body["expire"]
            .as_i64()
            .or_else(|| body["expires_in"].as_i64())
            .unwrap_or(7200)
            .max(120) as u64;
        Ok(AccessTokenCache {
            token,
            expires_at: Instant::now() + Duration::from_secs(expires_in.saturating_sub(60)),
        })
    }
}

async fn read_local_image_file(path: String, max_bytes: usize) -> crate::Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let path = Path::new(&path);
        let metadata = std::fs::symlink_metadata(path).map_err(|err| {
            crate::Error::from(anyhow::anyhow!("read image file metadata: {err}"))
        })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(anyhow::anyhow!("image path must not be a symlink").into());
        }
        if !file_type.is_file() {
            return Err(anyhow::anyhow!("image path must be a regular file").into());
        }
        if metadata.len() > max_bytes as u64 {
            return Err(anyhow::anyhow!("image file too large for upload").into());
        }
        let file = open_local_image_file(path)?;

        let mut bytes = Vec::with_capacity((metadata.len() as usize).min(max_bytes));
        file.take((max_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|err| crate::Error::from(anyhow::anyhow!("read image file: {err}")))?;
        if bytes.len() > max_bytes {
            return Err(anyhow::anyhow!("image file too large for upload").into());
        }
        Ok(bytes)
    })
    .await
    .map_err(|err| crate::Error::from(anyhow::anyhow!("join image file read task: {err}")))?
}

#[cfg(unix)]
fn open_local_image_file(path: &Path) -> crate::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|err| {
            if err.raw_os_error() == Some(libc::ELOOP) {
                crate::Error::from(anyhow::anyhow!("image path must not be a symlink"))
            } else {
                crate::Error::from(anyhow::anyhow!("read image file: {err}"))
            }
        })
}

#[cfg(not(unix))]
fn open_local_image_file(path: &Path) -> crate::Result<std::fs::File> {
    std::fs::File::open(path)
        .map_err(|err| crate::Error::from(anyhow::anyhow!("read image file: {err}")))
}

pub(super) fn read_bytes_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> BoxFuture<'static, crate::Result<Vec<u8>>> {
    Box::pin(async move {
        let mut stream = resp.bytes_stream();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| anyhow::anyhow!("read response bytes: {err}"))?;
            if out.len().saturating_add(chunk.len()) > max_bytes {
                return Err(anyhow::anyhow!("response body exceeds byte limit").into());
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    })
}

pub(super) fn guess_image_mime(ext: Option<&str>) -> String {
    match ext
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("")
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub(super) fn normalize_secret(secret: impl Into<String>) -> crate::Result<String> {
    let secret = secret.into();
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("feishu secret must not be empty").into());
    }
    Ok(secret.to_string())
}

pub(super) fn normalize_app_credentials(
    app_id: Option<String>,
    app_secret: Option<String>,
) -> crate::Result<Option<FeishuAppCredentials>> {
    let app_id = normalize_optional_trimmed(app_id, "app_id")?;
    let app_secret = normalize_optional_trimmed(app_secret, "app_secret")?;

    match (app_id, app_secret) {
        (None, None) => Ok(None),
        (Some(app_id), Some(app_secret)) => Ok(Some(FeishuAppCredentials { app_id, app_secret })),
        _ => Err(
            anyhow::anyhow!("feishu app credentials must include both app_id and app_secret")
                .into(),
        ),
    }
}

fn normalize_optional_trimmed(value: Option<String>, field: &str) -> crate::Result<Option<String>> {
    match value {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(anyhow::anyhow!("feishu {field} must not be empty").into());
            }
            Ok(Some(value.to_string()))
        }
        None => Ok(None),
    }
}
