use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use crate::SecretString;
use crate::log::{warn_feishu_image_load_failed, warn_feishu_image_upload_failed};
use crate::sinks::BoxFuture;
use http_kit::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, http_status_text_error, parse_and_validate_https_url_basic,
    read_json_body_limited, read_text_body_limited, response_body_read_error, send_reqwest,
};

use super::FeishuWebhookSink;

#[derive(Debug, Clone)]
pub(super) struct FeishuAppCredentials {
    pub(super) app_id: String,
    pub(super) app_secret: SecretString,
}

#[derive(Debug, Clone)]
pub(super) struct AccessTokenCache {
    pub(super) token: SecretString,
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

fn tenant_access_token_refresh_waiter(
    notify: std::sync::Arc<tokio::sync::Notify>,
) -> impl std::future::Future<Output = ()> {
    let mut notified = Box::pin(notify.notified_owned());

    // Register before releasing the state lock so a completing refresh cannot notify between the
    // state check and waiter installation.
    notified.as_mut().enable();
    async move {
        notified.await;
    }
}

impl FeishuWebhookSink {
    pub(super) async fn resolve_single_image_key(&self, src: &str) -> Option<String> {
        let media = self.media.as_ref()?;
        media.app_credentials.as_ref()?;
        if src.starts_with("https://") && !media.allow_remote_image_urls {
            return None;
        }

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
        let Some(media) = self.media.as_ref() else {
            return Err(anyhow::anyhow!("feishu media support is disabled").into());
        };
        if src.starts_with("https://") {
            if !media.allow_remote_image_urls {
                return Err(anyhow::anyhow!("remote image urls are disabled").into());
            }
            return self.load_remote_image(src).await;
        }

        if src.contains("://") {
            return Err(anyhow::anyhow!("unsupported image url scheme").into());
        }

        if !media.allow_local_image_files {
            return Err(anyhow::anyhow!("local image files are disabled").into());
        }

        let resolved_path = resolve_allowed_local_image_path(
            src,
            &media.local_image_roots,
            media.local_image_base_dir.as_deref(),
        )?;
        let bytes =
            read_local_image_file(resolved_path.clone(), media.image_upload_max_bytes).await?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("image file is empty").into());
        }
        if bytes.len() > media.image_upload_max_bytes {
            return Err(anyhow::anyhow!("image file too large for upload").into());
        }

        let file_name = resolved_path
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        let content_type = guess_image_mime(resolved_path.extension().and_then(|v| v.to_str()));

        Ok(LoadedImage {
            bytes,
            file_name,
            content_type,
        })
    }

    pub(super) async fn load_remote_image(&self, src: &str) -> crate::Result<LoadedImage> {
        let Some(media) = self.media.as_ref() else {
            return Err(anyhow::anyhow!("feishu media support is disabled").into());
        };
        let url = parse_and_validate_https_url_basic(src)?;
        let client = self.http.select_for_url(&url, true).await?;

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

        let bytes = read_bytes_body_limited(resp, media.image_upload_max_bytes).await?;
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

        let client = self
            .http
            .select_for_url(&upload_url, self.enforce_public_ip)
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
                .bearer_auth(access_token.expose_secret())
                .multipart(form),
            "feishu image upload",
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            self.invalidate_tenant_access_token(&access_token).await;
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
            self.invalidate_tenant_access_token(&access_token).await;
            return Err(anyhow::anyhow!("feishu image upload api error: code={code}").into());
        }

        let image_key = body["data"]["image_key"]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("feishu image upload api error: missing image_key"))?;

        Ok(image_key.to_string())
    }

    async fn invalidate_tenant_access_token(&self, token: &SecretString) {
        let Some(media) = self.media.as_ref() else {
            return;
        };
        let mut guard = media.tenant_access_token.lock().await;
        if matches!(
            &*guard,
            super::TenantAccessTokenState::Ready(cached)
                if cached.token.expose_secret() == token.expose_secret()
        ) {
            *guard = super::TenantAccessTokenState::Empty;
        }
    }

    pub(super) async fn ensure_tenant_access_token(&self) -> crate::Result<SecretString> {
        let Some(media) = self.media.as_ref() else {
            return Err(anyhow::anyhow!(
                "feishu app credentials are required for markdown image upload"
            )
            .into());
        };
        let Some(credentials) = media.app_credentials.as_ref().cloned() else {
            return Err(anyhow::anyhow!(
                "feishu app credentials are required for markdown image upload"
            )
            .into());
        };

        loop {
            let guard = media.tenant_access_token.lock().await;
            match &*guard {
                super::TenantAccessTokenState::Ready(cached)
                    if cached.expires_at > Instant::now() =>
                {
                    return Ok(cached.token.clone());
                }
                super::TenantAccessTokenState::Refreshing(notify) => {
                    let notify = std::sync::Arc::clone(notify);
                    let wait = tenant_access_token_refresh_waiter(notify);
                    drop(guard);
                    wait.await;
                    continue;
                }
                super::TenantAccessTokenState::Empty | super::TenantAccessTokenState::Ready(_) => {
                    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
                    drop(guard);

                    {
                        let mut guard = media.tenant_access_token.lock().await;
                        match &*guard {
                            super::TenantAccessTokenState::Ready(cached)
                                if cached.expires_at > Instant::now() =>
                            {
                                return Ok(cached.token.clone());
                            }
                            super::TenantAccessTokenState::Refreshing(existing) => {
                                let notify = std::sync::Arc::clone(existing);
                                let wait = tenant_access_token_refresh_waiter(notify);
                                drop(guard);
                                wait.await;
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
                        std::sync::Arc::clone(&media.tenant_access_token),
                        std::sync::Arc::clone(&notify),
                    );
                    let result = self.fetch_tenant_access_token(&credentials).await;
                    let mut guard = media.tenant_access_token.lock().await;
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

        let client = self
            .http
            .select_for_url(&token_url, self.enforce_public_ip)
            .await?;

        let payload = serde_json::json!({
            "app_id": credentials.app_id.as_str(),
            "app_secret": credentials.app_secret.expose_secret(),
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
            .ok_or_else(|| {
                anyhow::anyhow!("feishu tenant access token api error: missing token")
            })?;

        let expires_in = body["expire"]
            .as_i64()
            .or_else(|| body["expires_in"].as_i64())
            .unwrap_or(7200);
        Ok(AccessTokenCache {
            token: SecretString::new(token),
            expires_at: Instant::now() + tenant_access_token_cache_ttl(expires_in),
        })
    }
}

pub(super) fn normalize_local_image_roots(
    allow_local_image_files: bool,
    roots: Vec<PathBuf>,
) -> crate::Result<Vec<PathBuf>> {
    let normalized = roots
        .into_iter()
        .map(normalize_local_image_root)
        .collect::<crate::Result<Vec<_>>>()?;
    if allow_local_image_files && normalized.is_empty() {
        return Err(anyhow::anyhow!(
            "local image files require at least one configured local image root"
        )
        .into());
    }
    Ok(normalized)
}

pub(super) fn normalize_local_image_base_dir(
    base_dir: Option<PathBuf>,
) -> crate::Result<Option<PathBuf>> {
    base_dir.map(normalize_local_image_root).transpose()
}

fn normalize_local_image_root(root: PathBuf) -> crate::Result<PathBuf> {
    if !root.is_absolute() {
        return Err(anyhow::anyhow!(
            "local image roots must be absolute paths: {}",
            root.display()
        )
        .into());
    }

    let root = normalize_path(&root);

    #[cfg(unix)]
    ensure_local_image_path_has_no_symlink_components(&root)?;

    let metadata = std::fs::symlink_metadata(&root).map_err(|err| {
        crate::Error::from(anyhow::anyhow!("read local image root metadata: {err}"))
    })?;
    if !metadata.file_type().is_dir() {
        return Err(anyhow::anyhow!(
            "local image root must be an existing directory: {}",
            root.display()
        )
        .into());
    }

    Ok(root)
}

fn resolve_allowed_local_image_path(
    src: &str,
    roots: &[PathBuf],
    base_dir: Option<&Path>,
) -> crate::Result<PathBuf> {
    if roots.is_empty() {
        return Err(anyhow::anyhow!(
            "local image files require at least one configured local image root"
        )
        .into());
    }

    let resolved = resolve_local_image_path(Path::new(src), base_dir)?;
    if roots.iter().any(|root| resolved.starts_with(root)) {
        return Ok(resolved);
    }

    Err(anyhow::anyhow!(
        "image path is outside configured local image roots: {}",
        resolved.display()
    )
    .into())
}

fn resolve_local_image_path(path: &Path, base_dir: Option<&Path>) -> crate::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(resolve_local_image_path_with_base(Path::new("/"), path));
    }

    let Some(base_dir) = base_dir else {
        return Err(anyhow::anyhow!("relative local image paths require explicit base dir").into());
    };
    Ok(resolve_local_image_path_with_base(base_dir, path))
}

fn resolve_local_image_path_with_base(base: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    normalize_path(&absolute)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.parent().is_some() {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

async fn read_local_image_file(path: PathBuf, max_bytes: usize) -> crate::Result<Vec<u8>> {
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = max_bytes;
        return Err(anyhow::anyhow!("local image files are not supported on this platform").into());
    }

    #[cfg(unix)]
    tokio::task::spawn_blocking(move || {
        ensure_local_image_path_has_no_symlink_components(&path)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|err| {
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
        let file = open_local_image_file(&path)?;

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

fn tenant_access_token_cache_ttl(expires_in: i64) -> Duration {
    let expires_in = u64::try_from(expires_in.max(0)).unwrap_or_default();
    Duration::from_secs(expires_in.saturating_sub(60))
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

#[cfg(unix)]
fn ensure_local_image_path_has_no_symlink_components(path: &Path) -> crate::Result<()> {
    let mut current = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(part) => {
                current.push(part);
                let metadata = std::fs::symlink_metadata(&current).map_err(|err| {
                    crate::Error::from(anyhow::anyhow!(
                        "read image path metadata for {}: {err}",
                        current.display()
                    ))
                })?;
                if metadata.file_type().is_symlink() {
                    return Err(anyhow::anyhow!(
                        "image path must not traverse symlink component: {}",
                        current.display()
                    )
                    .into());
                }
            }
        }
    }

    Ok(())
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

pub(super) fn normalize_secret(secret: impl Into<SecretString>) -> crate::Result<SecretString> {
    let secret = secret.into();
    let secret = secret.expose_secret().trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("feishu secret must not be empty").into());
    }
    Ok(SecretString::new(secret))
}

pub(super) fn normalize_app_credentials(
    app_id: Option<String>,
    app_secret: Option<SecretString>,
) -> crate::Result<Option<FeishuAppCredentials>> {
    let app_id = normalize_optional_trimmed(app_id, "app_id")?;
    let app_secret = normalize_optional_secret(app_secret, "app_secret")?;

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

fn normalize_optional_secret(
    value: Option<SecretString>,
    field: &str,
) -> crate::Result<Option<SecretString>> {
    match value {
        Some(value) => {
            let value = value.expose_secret().trim();
            if value.is_empty() {
                return Err(anyhow::anyhow!("feishu {field} must not be empty").into());
            }
            Ok(Some(SecretString::new(value)))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        ensure_local_image_path_has_no_symlink_components, normalize_local_image_base_dir,
        normalize_local_image_roots, resolve_local_image_path, resolve_local_image_path_with_base,
        tenant_access_token_cache_ttl, tenant_access_token_refresh_waiter,
    };

    #[tokio::test]
    async fn tenant_access_token_refresh_waiter_handles_notify_before_await() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let wait = tenant_access_token_refresh_waiter(Arc::clone(&notify));

        notify.notify_waiters();

        tokio::time::timeout(Duration::from_millis(50), wait)
            .await
            .expect("enabled waiter should observe notify_waiters before await");
    }

    #[test]
    fn tenant_access_token_cache_ttl_does_not_extend_short_server_ttls() {
        assert_eq!(tenant_access_token_cache_ttl(30), Duration::ZERO);
        assert_eq!(tenant_access_token_cache_ttl(120), Duration::from_secs(60));
        assert_eq!(tenant_access_token_cache_ttl(-1), Duration::ZERO);
    }

    #[test]
    fn resolve_local_image_path_with_base_normalizes_parent_segments() {
        let resolved = resolve_local_image_path_with_base(
            Path::new("/workspace/project/run"),
            Path::new("../images/./demo.png"),
        );
        assert_eq!(
            resolved,
            PathBuf::from("/workspace/project/images/demo.png")
        );
    }

    #[test]
    fn normalize_local_image_roots_rejects_relative_roots() {
        let err = normalize_local_image_roots(true, vec![PathBuf::from("relative-root")])
            .expect_err("relative roots should be rejected");
        assert!(err.to_string().contains("absolute paths"), "{err:#}");
    }

    #[test]
    fn normalize_local_image_base_dir_rejects_relative_paths() {
        let err = normalize_local_image_base_dir(Some(PathBuf::from("relative-root")))
            .expect_err("relative base dir should be rejected");
        assert!(err.to_string().contains("absolute paths"), "{err:#}");
    }

    #[test]
    fn resolve_local_image_path_requires_explicit_base_dir_for_relative_paths() {
        let err = resolve_local_image_path(Path::new("image.png"), None)
            .expect_err("relative path without base dir should fail closed");
        assert!(
            err.to_string()
                .contains("relative local image paths require explicit base dir"),
            "{err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_component_check_catches_parent_relative_escape_to_symlink() {
        let base = std::env::temp_dir().join(format!(
            "notify-kit-feishu-media-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let cwd = base.join("cwd");
        let target = base.join("target");
        let link = base.join("link");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        std::fs::create_dir_all(&target).expect("create target");
        std::fs::write(target.join("image.png"), b"png").expect("write image");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        let resolved = resolve_local_image_path_with_base(&cwd, Path::new("../link/image.png"));
        let err = ensure_local_image_path_has_no_symlink_components(&resolved)
            .expect_err("parent-relative symlink path should be rejected");
        assert!(err.to_string().contains("symlink component"), "{err:#}");

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&base);
    }
}
