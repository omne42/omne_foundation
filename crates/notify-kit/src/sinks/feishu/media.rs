#[cfg(unix)]
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
#[cfg(unix)]
use omne_fs_primitives::{
    MissingRootPolicy, open_directory_component, open_regular_file_at, open_root,
    read_to_end_limited,
};

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

#[derive(Debug, Clone)]
struct AllowedLocalImagePath {
    absolute: PathBuf,
    root: PathBuf,
    relative: PathBuf,
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

        reset_tenant_access_token_refresh_state(
            std::sync::Arc::clone(&self.state),
            std::sync::Arc::clone(&self.notify),
        );
    }
}

async fn clear_tenant_access_token_refresh_state(
    state: std::sync::Arc<tokio::sync::Mutex<super::TenantAccessTokenState>>,
    notify: std::sync::Arc<tokio::sync::Notify>,
) {
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
}

fn reset_tenant_access_token_refresh_state(
    state: std::sync::Arc<tokio::sync::Mutex<super::TenantAccessTokenState>>,
    notify: std::sync::Arc<tokio::sync::Notify>,
) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        drop(handle.spawn(clear_tenant_access_token_refresh_state(state, notify)));
        return;
    }

    if let Ok(mut guard) = state.try_lock() {
        if matches!(
            &*guard,
            super::TenantAccessTokenState::Refreshing(current)
                if std::sync::Arc::ptr_eq(current, &notify)
        ) {
            *guard = super::TenantAccessTokenState::Empty;
        }
        drop(guard);
        notify.notify_waiters();
        return;
    }

    let _ = std::thread::Builder::new()
        .name("notify-kit-feishu-refresh-reset".to_string())
        .spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(clear_tenant_access_token_refresh_state(state, notify));
            }
        });
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
        self.app_credentials.as_ref()?;
        if src.starts_with("https://") && !self.allow_remote_image_urls {
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
        if src.starts_with("https://") {
            if !self.allow_remote_image_urls {
                return Err(anyhow::anyhow!("remote image urls are disabled").into());
            }
            return self.load_remote_image(src).await;
        }

        if src.contains("://") {
            return Err(anyhow::anyhow!("unsupported image url scheme").into());
        }

        if !self.allow_local_image_files {
            return Err(anyhow::anyhow!("local image files are disabled").into());
        }

        let resolved_path = resolve_allowed_local_image_path(
            src,
            self.local_image_base_dir.as_deref(),
            &self.local_image_roots,
        )?;
        let bytes = read_local_image_file(&resolved_path, self.image_upload_max_bytes).await?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("image file is empty").into());
        }
        if bytes.len() > self.image_upload_max_bytes {
            return Err(anyhow::anyhow!("image file too large for upload").into());
        }

        let file_name = resolved_path
            .absolute
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        finalize_loaded_image(bytes, file_name, "image file")
    }

    pub(super) async fn load_remote_image(&self, src: &str) -> crate::Result<LoadedImage> {
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

        let bytes = read_bytes_body_limited(resp, self.image_upload_max_bytes).await?;
        let file_name = Path::new(url.path())
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        finalize_loaded_image(bytes, file_name, "downloaded image")
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
        let mut guard = self.tenant_access_token.lock().await;
        if matches!(
            &*guard,
            super::TenantAccessTokenState::Ready(cached)
                if cached.token.expose_secret() == token.expose_secret()
        ) {
            *guard = super::TenantAccessTokenState::Empty;
        }
    }

    pub(super) async fn ensure_tenant_access_token(&self) -> crate::Result<SecretString> {
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
                    let wait = tenant_access_token_refresh_waiter(notify);
                    drop(guard);
                    wait.await;
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
    base_dir.map(normalize_local_image_base).transpose()
}

fn normalize_local_image_base(base_dir: PathBuf) -> crate::Result<PathBuf> {
    if !base_dir.is_absolute() {
        return Err(anyhow::anyhow!(
            "local image base dir must be absolute: {}",
            base_dir.display()
        )
        .into());
    }

    Ok(normalize_path(&base_dir))
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
    {
        let _ = open_root(
            &root,
            "local image root",
            MissingRootPolicy::Error,
            |_, component, root, error| map_local_image_root_open_error(root, component, error),
        )
        .map_err(|err| crate::Error::from(anyhow::anyhow!("{err}")))?;
    }

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
    base_dir: Option<&Path>,
    roots: &[PathBuf],
) -> crate::Result<AllowedLocalImagePath> {
    if roots.is_empty() {
        return Err(anyhow::anyhow!(
            "local image files require at least one configured local image root"
        )
        .into());
    }

    let resolved = resolve_local_image_path(Path::new(src), base_dir)?;
    for root in roots {
        let Ok(relative) = resolved.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_path_buf();
        if relative.as_os_str().is_empty() {
            return Err(anyhow::anyhow!(
                "image path must reference a file within configured local image roots: {}",
                resolved.display()
            )
            .into());
        }
        return Ok(AllowedLocalImagePath {
            absolute: resolved.clone(),
            root: root.clone(),
            relative,
        });
    }

    Err(anyhow::anyhow!(
        "image path is outside configured local image roots: {}",
        resolved.display()
    )
    .into())
}

fn resolve_local_image_path(path: &Path, base_dir: Option<&Path>) -> crate::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(normalize_path(path));
    }

    let Some(base_dir) = base_dir else {
        return Err(anyhow::anyhow!(
            "relative local image paths require explicit local image base dir"
        )
        .into());
    };

    if !base_dir.is_absolute() {
        return Err(anyhow::anyhow!(
            "local image base dir must be absolute: {}",
            base_dir.display()
        )
        .into());
    }

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

async fn read_local_image_file(
    path: &AllowedLocalImagePath,
    max_bytes: usize,
) -> crate::Result<Vec<u8>> {
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = max_bytes;
        return Err(anyhow::anyhow!("local image files are not supported on this platform").into());
    }

    #[cfg(unix)]
    {
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            let mut file = open_allowed_local_image_file(&path)?;
            let (bytes, truncated) = read_to_end_limited(&mut file, max_bytes)
                .map_err(|err| crate::Error::from(anyhow::anyhow!("read image file: {err}")))?;
            if truncated {
                return Err(anyhow::anyhow!("image file too large for upload").into());
            }
            Ok(bytes)
        })
        .await
        .map_err(|err| crate::Error::from(anyhow::anyhow!("join image file read task: {err}")))?
    }
}

fn tenant_access_token_cache_ttl(expires_in: i64) -> Duration {
    let expires_in = u64::try_from(expires_in.max(0)).unwrap_or_default();
    Duration::from_secs(expires_in.saturating_sub(60))
}

#[cfg(unix)]
fn open_allowed_local_image_file(
    path: &AllowedLocalImagePath,
) -> crate::Result<omne_fs_primitives::File> {
    let Some(root) = open_root(
        &path.root,
        "local image root",
        MissingRootPolicy::Error,
        |_, component, root, error| map_local_image_root_open_error(root, component, error),
    )
    .map_err(|err| crate::Error::from(anyhow::anyhow!("{err}")))?
    else {
        return Err(
            anyhow::anyhow!("local image root does not exist: {}", path.root.display()).into(),
        );
    };

    let mut directory = root.into_dir();
    let mut traversed = PathBuf::new();
    let mut components = path.relative.components().peekable();
    let file_name = loop {
        match components.next() {
            Some(Component::Normal(component)) if components.peek().is_none() => {
                break PathBuf::from(component);
            }
            Some(Component::Normal(component)) => {
                traversed.push(component);
                directory =
                    open_directory_component(&directory, Path::new(component)).map_err(|err| {
                        map_local_image_component_open_error(&path.root, &traversed, err)
                    })?;
            }
            Some(other) => {
                return Err(anyhow::anyhow!(
                    "image path contains unsupported component {other:?}: {}",
                    path.absolute.display()
                )
                .into());
            }
            None => {
                return Err(anyhow::anyhow!(
                    "image path must reference a file: {}",
                    path.absolute.display()
                )
                .into());
            }
        }
    };

    open_regular_file_at(&directory, &file_name)
        .map_err(|err| map_local_image_file_open_error(&path.absolute, err))
}

#[cfg(unix)]
fn map_local_image_root_open_error(root: &Path, component: &Path, error: io::Error) -> io::Error {
    let target = root.join(component);
    if omne_fs_primitives::is_symlink_or_reparse_open_error(&error) {
        return io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "local image root must not traverse symlink component: {}",
                target.display()
            ),
        );
    }
    io::Error::new(
        error.kind(),
        format!(
            "open local image root component {}: {error}",
            target.display()
        ),
    )
}

#[cfg(unix)]
fn map_local_image_component_open_error(
    root: &Path,
    traversed: &Path,
    error: io::Error,
) -> crate::Error {
    let target = root.join(traversed);
    if target_is_symlink(&target) {
        return anyhow::anyhow!(
            "image path must not traverse symlink component: {}",
            target.display()
        )
        .into();
    }
    if omne_fs_primitives::is_symlink_or_reparse_open_error(&error) {
        return anyhow::anyhow!(
            "image path must not traverse symlink component: {}",
            target.display()
        )
        .into();
    }
    anyhow::anyhow!("open image path component {}: {error}", target.display()).into()
}

#[cfg(unix)]
fn map_local_image_file_open_error(path: &Path, error: io::Error) -> crate::Error {
    if target_is_symlink(path) {
        return anyhow::anyhow!("image path must not be a symlink: {}", path.display()).into();
    }
    if omne_fs_primitives::is_symlink_or_reparse_open_error(&error) {
        return anyhow::anyhow!("image path must not be a symlink: {}", path.display()).into();
    }
    if error.kind() == io::ErrorKind::InvalidInput {
        return anyhow::anyhow!("image path must be a regular file: {}", path.display()).into();
    }
    anyhow::anyhow!("read image file: {error}").into()
}

#[cfg(unix)]
fn target_is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
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

fn finalize_loaded_image(
    bytes: Vec<u8>,
    file_name: String,
    source: &'static str,
) -> crate::Result<LoadedImage> {
    if bytes.is_empty() {
        return Err(anyhow::anyhow!("{source} is empty").into());
    }

    let Some(content_type) = detect_image_content_type(&bytes) else {
        return Err(anyhow::anyhow!("{source} is not a supported image format").into());
    };

    Ok(LoadedImage {
        bytes,
        file_name,
        content_type: content_type.to_string(),
    })
}

fn detect_image_content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if is_heic_image(bytes) {
        return Some("image/heic");
    }
    None
}

fn is_heic_image(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }

    matches!(
        &bytes[8..12],
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs"
    )
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
        TenantAccessTokenRefreshGuard, detect_image_content_type, finalize_loaded_image,
        normalize_local_image_base_dir, normalize_local_image_roots,
        resolve_allowed_local_image_path, resolve_local_image_path,
        resolve_local_image_path_with_base, tenant_access_token_cache_ttl,
        tenant_access_token_refresh_waiter,
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
    fn tenant_access_token_refresh_guard_resets_state_without_runtime() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let wait = tenant_access_token_refresh_waiter(Arc::clone(&notify));
        let state = Arc::new(tokio::sync::Mutex::new(
            super::super::TenantAccessTokenState::Refreshing(Arc::clone(&notify)),
        ));

        drop(TenantAccessTokenRefreshGuard::new(
            Arc::clone(&state),
            Arc::clone(&notify),
        ));

        let guard = state.blocking_lock();
        assert!(
            matches!(&*guard, super::super::TenantAccessTokenState::Empty),
            "guard drop without runtime should clear Refreshing state"
        );
        drop(guard);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(50), wait)
                .await
                .expect("guard drop should notify waiters without a runtime");
        });
    }

    #[test]
    fn tenant_access_token_refresh_guard_waits_for_locked_state_without_runtime() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let wait = tenant_access_token_refresh_waiter(Arc::clone(&notify));
        let state = Arc::new(tokio::sync::Mutex::new(
            super::super::TenantAccessTokenState::Refreshing(Arc::clone(&notify)),
        ));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let held_guard = state.blocking_lock();

        drop(TenantAccessTokenRefreshGuard::new(
            Arc::clone(&state),
            Arc::clone(&notify),
        ));

        assert!(
            matches!(
                &*held_guard,
                super::super::TenantAccessTokenState::Refreshing(current)
                    if Arc::ptr_eq(current, &notify)
            ),
            "held lock should keep the refresh state unchanged until released"
        );
        drop(held_guard);

        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(1), wait)
                .await
                .expect("background reset should notify waiters after the lock is released");

            let guard = state.lock().await;
            assert!(
                matches!(&*guard, super::super::TenantAccessTokenState::Empty),
                "background reset should clear Refreshing state once the lock becomes available"
            );
        });
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
        let err = normalize_local_image_base_dir(Some(PathBuf::from("relative-base")))
            .expect_err("relative base dirs should be rejected");
        assert!(
            err.to_string().contains("base dir must be absolute"),
            "{err:#}"
        );
    }

    #[test]
    fn resolve_local_image_path_requires_explicit_base_for_relative_paths() {
        let err = resolve_local_image_path(Path::new("image.png"), None)
            .expect_err("relative local image path should require explicit base");
        assert!(
            err.to_string()
                .contains("relative local image paths require explicit local image base dir"),
            "{err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_allowed_local_image_path_preserves_root_relative_boundary() {
        let root = PathBuf::from("/workspace/images");
        let resolved = resolve_allowed_local_image_path(
            "../demo.png",
            Some(Path::new("/workspace/images/nested")),
            std::slice::from_ref(&root),
        )
        .expect("resolve allowed local image path");

        assert_eq!(resolved.root, root);
        assert_eq!(resolved.relative, PathBuf::from("demo.png"));
        assert_eq!(
            resolved.absolute,
            PathBuf::from("/workspace/images/demo.png")
        );
    }

    #[test]
    fn detect_image_content_type_recognizes_supported_formats() {
        assert_eq!(
            detect_image_content_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(
            detect_image_content_type(b"\xff\xd8\xff\xe0rest"),
            Some("image/jpeg")
        );
        assert_eq!(detect_image_content_type(b"GIF89arest"), Some("image/gif"));
        assert_eq!(
            detect_image_content_type(b"RIFF\x00\x00\x00\x00WEBPrest"),
            Some("image/webp")
        );
        assert_eq!(detect_image_content_type(b"BMrest"), Some("image/bmp"));
        assert_eq!(
            detect_image_content_type(b"\x00\x00\x00\x18ftypheicrest"),
            Some("image/heic")
        );
    }

    #[test]
    fn finalize_loaded_image_rejects_non_image_bytes_even_with_image_name() {
        let err = finalize_loaded_image(
            b"not actually an image".to_vec(),
            "masquerade.png".to_string(),
            "image file",
        )
        .expect_err("non-image bytes should fail closed");

        assert!(
            err.to_string()
                .contains("image file is not a supported image format"),
            "{err:#}"
        );
    }

    #[test]
    fn finalize_loaded_image_uses_detected_content_type_over_file_name() {
        let loaded = finalize_loaded_image(
            b"\xff\xd8\xff\xe0rest".to_vec(),
            "photo.png".to_string(),
            "image file",
        )
        .expect("jpeg bytes should be accepted");

        assert_eq!(loaded.file_name, "photo.png");
        assert_eq!(loaded.content_type, "image/jpeg");
    }
}
