use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as TokioMutex, RwLock, Semaphore};

use crate::public_ip::validate_public_addrs;

const DEFAULT_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT: usize = 32;
// Re-resolve DNS on every pinned-client selection instead of reusing a cross-request pinned
// client cache entry, so failover or rebinding cannot keep routing requests through a stale
// address set for an arbitrary TTL window.
const DEFAULT_PINNED_CLIENT_TTL: Duration = Duration::ZERO;
const DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PinnedRedirectOrigin {
    host: String,
    scheme: String,
    port: u16,
}

impl PinnedRedirectOrigin {
    fn from_url(url: &reqwest::Url) -> crate::Result<Self> {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!("url must have an explicit or known default port"))?;
        Ok(Self {
            host: host.to_string(),
            scheme: url.scheme().to_string(),
            port,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PinnedClientOptionsKey {
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    follow_redirects: bool,
    no_proxy: bool,
    default_headers: Vec<(String, Vec<u8>)>,
}

impl PinnedClientOptionsKey {
    fn from_pinned_options(options: &HttpClientOptions) -> Self {
        Self::from_options_with_no_proxy(options, true)
    }

    fn from_options_with_no_proxy(options: &HttpClientOptions, no_proxy: bool) -> Self {
        let mut default_headers = options
            .default_headers
            .iter()
            .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
            .collect::<Vec<_>>();
        default_headers.sort();
        Self {
            timeout: options.timeout,
            connect_timeout: options.connect_timeout,
            follow_redirects: options.follow_redirects,
            no_proxy,
            default_headers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PinnedClientKey {
    host: String,
    scheme: String,
    port: u16,
    options: PinnedClientOptionsKey,
}

#[derive(Clone)]
struct CachedPinnedClient {
    client: reqwest::Client,
    expires_at: Instant,
}

static PINNED_CLIENT_CACHE: OnceLock<RwLock<HashMap<PinnedClientKey, CachedPinnedClient>>> =
    OnceLock::new();
static PINNED_CLIENT_BUILD_LOCKS: OnceLock<Mutex<HashMap<PinnedClientKey, Weak<TokioMutex<()>>>>> =
    OnceLock::new();
static DNS_LOOKUP_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static DNS_LOOKUP_TIMEOUT_MESSAGE: OnceLock<String> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct HttpClientOptions {
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub default_headers: reqwest::header::HeaderMap,
    pub follow_redirects: bool,
    pub no_proxy: bool,
}

impl Default for HttpClientOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            connect_timeout: None,
            default_headers: reqwest::header::HeaderMap::new(),
            follow_redirects: false,
            no_proxy: false,
        }
    }
}

#[derive(Clone)]
pub struct HttpClientProfile {
    client: reqwest::Client,
    options: HttpClientOptions,
}

impl HttpClientProfile {
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    #[must_use]
    pub fn options(&self) -> &HttpClientOptions {
        &self.options
    }

    pub async fn select_for_url(
        &self,
        url: &reqwest::Url,
        enforce_public_ip: bool,
    ) -> crate::Result<reqwest::Client> {
        select_http_client_from_profile(self, url, enforce_public_ip).await
    }
}

fn dns_lookup_timeout_message() -> &'static str {
    DNS_LOOKUP_TIMEOUT_MESSAGE
        .get_or_init(|| format!("dns lookup timeout (capped at {DEFAULT_DNS_LOOKUP_TIMEOUT:?})"))
        .as_str()
}

fn pinned_client_cache() -> &'static RwLock<HashMap<PinnedClientKey, CachedPinnedClient>> {
    PINNED_CLIENT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn pinned_client_cache_reuse_enabled() -> bool {
    DEFAULT_PINNED_CLIENT_TTL > Duration::ZERO
}

fn pinned_client_build_locks() -> &'static Mutex<HashMap<PinnedClientKey, Weak<TokioMutex<()>>>> {
    PINNED_CLIENT_BUILD_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pinned_client_build_locks()
-> std::sync::MutexGuard<'static, HashMap<PinnedClientKey, Weak<TokioMutex<()>>>> {
    pinned_client_build_locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn cleanup_pinned_client_build_lock_entry(key: &PinnedClientKey) {
    let mut locks = lock_pinned_client_build_locks();
    if locks.get(key).is_some_and(|weak| weak.strong_count() == 0) {
        locks.remove(key);
    }
}

struct PinnedClientBuildLockCleanupGuard {
    key: PinnedClientKey,
    armed: bool,
}

impl PinnedClientBuildLockCleanupGuard {
    fn new(key: PinnedClientKey) -> Self {
        Self { key, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PinnedClientBuildLockCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            cleanup_pinned_client_build_lock_entry(&self.key);
        }
    }
}

fn dns_lookup_semaphore() -> &'static Arc<Semaphore> {
    DNS_LOOKUP_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT)))
}

fn remaining_dns_timeout(deadline: Instant) -> crate::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining == Duration::ZERO {
        return Err(anyhow::anyhow!(dns_lookup_timeout_message()).into());
    }
    Ok(remaining)
}

fn cap_pinned_client_cache_entries(
    cache: &mut HashMap<PinnedClientKey, CachedPinnedClient>,
    max: usize,
    keep: &PinnedClientKey,
) {
    if max == 0 {
        cache.clear();
        return;
    }

    while cache.len() > max {
        let Some(key) = cache
            .iter()
            .filter(|(key, _)| *key != keep)
            .min_by_key(|(key, value)| (value.expires_at, (*key).clone()))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&key);
    }
}

fn pinned_client_key(
    options: &HttpClientOptions,
    url: &reqwest::Url,
) -> crate::Result<PinnedClientKey> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("url must have an explicit or known default port"))?;
    Ok(PinnedClientKey {
        host: host.to_string(),
        scheme: url.scheme().to_string(),
        port,
        options: PinnedClientOptionsKey::from_pinned_options(options),
    })
}

fn redirect_policy(follow_redirects: bool) -> reqwest::redirect::Policy {
    if follow_redirects {
        reqwest::redirect::Policy::limited(10)
    } else {
        reqwest::redirect::Policy::none()
    }
}

fn pinned_redirect_policy(url: &reqwest::Url, follow_redirects: bool) -> reqwest::redirect::Policy {
    if !follow_redirects {
        return reqwest::redirect::Policy::none();
    }

    let Ok(initial_origin) = PinnedRedirectOrigin::from_url(url) else {
        return reqwest::redirect::Policy::none();
    };

    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }

        let Ok(next_origin) = PinnedRedirectOrigin::from_url(attempt.url()) else {
            return attempt.error("redirect target origin invalid");
        };

        if !next_origin.host.eq_ignore_ascii_case(&initial_origin.host) {
            return attempt.error("redirect target host changed under public-ip pinning");
        }

        if !next_origin
            .scheme
            .eq_ignore_ascii_case(&initial_origin.scheme)
        {
            return attempt.error("redirect target scheme changed under public-ip pinning");
        }

        if next_origin.port != initial_origin.port {
            return attempt.error("redirect target port changed under public-ip pinning");
        }

        attempt.follow()
    })
}

fn build_http_client_builder_with_policy(
    options: &HttpClientOptions,
    redirect_policy: reqwest::redirect::Policy,
    disable_env_proxy: bool,
) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .redirect(redirect_policy)
        .default_headers(options.default_headers.clone());

    if disable_env_proxy || options.no_proxy {
        builder = builder.no_proxy();
    }
    if let Some(timeout) = options.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect_timeout) = options.connect_timeout {
        builder = builder.connect_timeout(connect_timeout);
    }

    builder
}

fn build_http_client_builder(options: &HttpClientOptions) -> reqwest::ClientBuilder {
    build_http_client_builder_with_policy(options, redirect_policy(options.follow_redirects), false)
}

pub fn build_http_client(timeout: Duration) -> crate::Result<reqwest::Client> {
    build_http_client_with_options(&HttpClientOptions {
        timeout: Some(timeout),
        ..Default::default()
    })
}

pub fn build_http_client_with_options(
    options: &HttpClientOptions,
) -> crate::Result<reqwest::Client> {
    build_http_client_builder(options)
        .build()
        .map_err(|err| anyhow::anyhow!("build reqwest client: {err}").into())
}

pub fn build_http_client_profile(options: &HttpClientOptions) -> crate::Result<HttpClientProfile> {
    Ok(HttpClientProfile {
        client: build_http_client_with_options(options)?,
        options: options.clone(),
    })
}

pub(crate) fn sanitize_reqwest_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_request() {
        "request"
    } else if err.is_decode() {
        "decode"
    } else {
        "unknown"
    }
}

pub async fn send_reqwest(
    builder: reqwest::RequestBuilder,
    context: &str,
) -> crate::Result<reqwest::Response> {
    builder.send().await.map_err(|err| {
        anyhow::anyhow!(
            "{context} request failed ({})",
            sanitize_reqwest_error(&err)
        )
        .into()
    })
}

async fn resolve_url_to_public_addrs_async(
    url: &reqwest::Url,
    timeout: Duration,
) -> crate::Result<Vec<SocketAddr>> {
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host").into());
    };

    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("url must have an explicit or known default port"))?;
    let dns_timeout = timeout.min(DEFAULT_DNS_LOOKUP_TIMEOUT);
    if dns_timeout == Duration::ZERO {
        return Err(anyhow::anyhow!(dns_lookup_timeout_message()).into());
    }

    let deadline = Instant::now() + dns_timeout;
    let lookup = {
        let _permit = tokio::time::timeout(
            remaining_dns_timeout(deadline)?,
            dns_lookup_semaphore().acquire(),
        )
        .await
        .map_err(|_| anyhow::anyhow!(dns_lookup_timeout_message()))?
        .map_err(|_| anyhow::anyhow!("dns lookup failed"))?;

        tokio::time::timeout(
            remaining_dns_timeout(deadline)?,
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| anyhow::anyhow!(dns_lookup_timeout_message()))?
        .map_err(|err| anyhow::anyhow!("dns lookup failed: {err}"))?
    };

    validate_public_addrs(lookup)
}

fn resolve_override_addrs_for_reqwest(url: &reqwest::Url, addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    if url.port().is_some() {
        return addrs.to_vec();
    }

    addrs
        .iter()
        .copied()
        .map(|mut addr| {
            addr.set_port(0);
            addr
        })
        .collect()
}

fn dns_lookup_timeout_for_options(options: &HttpClientOptions) -> Duration {
    match (options.timeout, options.connect_timeout) {
        (Some(timeout), Some(connect_timeout)) => timeout.min(connect_timeout),
        (Some(timeout), None) => timeout,
        (None, Some(connect_timeout)) => connect_timeout,
        (None, None) => DEFAULT_DNS_LOOKUP_TIMEOUT,
    }
}

fn build_http_client_pinned_with_addrs(
    options: &HttpClientOptions,
    url: &reqwest::Url,
    addrs: &[SocketAddr],
) -> crate::Result<reqwest::Client> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;
    let addrs = resolve_override_addrs_for_reqwest(url, addrs);
    build_http_client_builder_with_policy(
        options,
        pinned_redirect_policy(url, options.follow_redirects),
        true,
    )
    .resolve_to_addrs(host, &addrs)
    .build()
    .map_err(|err| anyhow::anyhow!("build reqwest client: {err}").into())
}

async fn build_http_client_pinned_async(
    options: &HttpClientOptions,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    let lookup_timeout = dns_lookup_timeout_for_options(options);
    let addrs = resolve_url_to_public_addrs_async(url, lookup_timeout).await?;
    build_http_client_pinned_with_addrs(options, url, &addrs)
}

async fn select_pinned_http_client_with_options(
    options: &HttpClientOptions,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    let key = pinned_client_key(options, url)?;
    if pinned_client_cache_reuse_enabled() {
        let lookup_now = Instant::now();
        let should_cleanup_expired_cache_entry = {
            let cache = pinned_client_cache().read().await;
            match cache.get(&key) {
                Some(cached) if cached.expires_at > lookup_now => return Ok(cached.client.clone()),
                Some(_) => true,
                None => false,
            }
        };

        if should_cleanup_expired_cache_entry {
            let mut cache = pinned_client_cache().write().await;
            let now = Instant::now();
            if cache
                .get(&key)
                .is_some_and(|cached| cached.expires_at <= now)
            {
                cache.remove(&key);
            }
        }
    } else {
        let mut cache = pinned_client_cache().write().await;
        cache.remove(&key);
    }

    let mut build_lock_cleanup = PinnedClientBuildLockCleanupGuard::new(key.clone());
    let key_lock = {
        let mut locks = lock_pinned_client_build_locks();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(existing) = locks.get(&key).and_then(Weak::upgrade) {
            existing
        } else {
            let new_lock = Arc::new(TokioMutex::new(()));
            locks.insert(key.clone(), Arc::downgrade(&new_lock));
            new_lock
        }
    };

    let result: crate::Result<reqwest::Client> = async {
        let _build_guard = key_lock.lock().await;
        if pinned_client_cache_reuse_enabled() {
            let now = Instant::now();
            let cached_client = {
                let cache = pinned_client_cache().read().await;
                cache.get(&key).and_then(|cached| {
                    if cached.expires_at > now {
                        Some(cached.client.clone())
                    } else {
                        None
                    }
                })
            };
            if let Some(client) = cached_client {
                return Ok(client);
            }

            let client = build_http_client_pinned_async(options, url).await?;
            let now = Instant::now();
            {
                let mut cache = pinned_client_cache().write().await;
                cache.retain(|_, v| v.expires_at > now);
                cache.insert(
                    key.clone(),
                    CachedPinnedClient {
                        client: client.clone(),
                        expires_at: now + DEFAULT_PINNED_CLIENT_TTL,
                    },
                );
                cap_pinned_client_cache_entries(
                    &mut cache,
                    DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES,
                    &key,
                );
            }
            Ok(client)
        } else {
            build_http_client_pinned_async(options, url).await
        }
    }
    .await;

    drop(key_lock);
    cleanup_pinned_client_build_lock_entry(&key);
    build_lock_cleanup.disarm();

    result
}

pub async fn select_http_client_from_profile(
    profile: &HttpClientProfile,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> crate::Result<reqwest::Client> {
    if !enforce_public_ip {
        return Ok(profile.client.clone());
    }

    // The pinned path disables proxy resolution so the actual socket still targets the
    // DNS-validated address set instead of an intermediate proxy endpoint.
    select_pinned_http_client_with_options(&profile.options, url).await
}

pub async fn select_http_client_with_options(
    base_client: &reqwest::Client,
    options: &HttpClientOptions,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> crate::Result<reqwest::Client> {
    if !enforce_public_ip {
        return Ok(base_client.clone());
    }

    // `reqwest::Client` does not expose a safe way to clone its opaque builder state while
    // swapping in per-host DNS pinning. Callers that need the same configuration on both paths
    // should prefer `HttpClientProfile`, which keeps the reusable options explicit.
    select_pinned_http_client_with_options(options, url).await
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn timeout_only_options(timeout: Duration) -> HttpClientOptions {
        HttpClientOptions {
            timeout: Some(timeout),
            ..Default::default()
        }
    }

    fn pinned_key_for_timeout(url: &reqwest::Url, timeout: Duration) -> PinnedClientKey {
        pinned_client_key(&timeout_only_options(timeout), url).expect("build pinned client key")
    }

    #[test]
    fn dns_lookup_timeout_prefers_connect_timeout() {
        let timeout = dns_lookup_timeout_for_options(&HttpClientOptions {
            timeout: Some(Duration::from_secs(30)),
            connect_timeout: Some(Duration::from_millis(250)),
            ..Default::default()
        });
        assert_eq!(timeout, Duration::from_millis(250));

        let timeout = dns_lookup_timeout_for_options(&HttpClientOptions {
            timeout: Some(Duration::from_millis(500)),
            connect_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        assert_eq!(timeout, Duration::from_millis(500));
    }

    #[test]
    fn dns_lookup_timeout_falls_back_to_request_timeout_then_default() {
        let timeout = dns_lookup_timeout_for_options(&HttpClientOptions {
            timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        });
        assert_eq!(timeout, Duration::from_secs(3));

        let default_timeout = dns_lookup_timeout_for_options(&HttpClientOptions::default());
        assert_eq!(default_timeout, DEFAULT_DNS_LOOKUP_TIMEOUT);
    }

    #[test]
    fn remaining_dns_timeout_accepts_future_deadline() {
        let remaining =
            remaining_dns_timeout(Instant::now() + Duration::from_millis(10)).expect("timeout");
        assert!(remaining > Duration::ZERO);
        assert!(remaining <= Duration::from_millis(10));
    }

    #[test]
    fn remaining_dns_timeout_rejects_elapsed_deadline() {
        let err =
            remaining_dns_timeout(Instant::now()).expect_err("elapsed deadline should be rejected");
        assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");
    }

    #[test]
    fn pinned_client_key_keeps_sub_millisecond_timeout_precision() {
        let url = reqwest::Url::parse("https://example.com/webhook").expect("parse url");
        let lhs = pinned_key_for_timeout(&url, Duration::from_micros(500));
        let rhs = pinned_key_for_timeout(&url, Duration::from_micros(900));
        assert_ne!(lhs, rhs);
    }

    #[test]
    fn pinned_client_key_distinguishes_port_and_default_headers() {
        let https = reqwest::Url::parse("https://example.com/webhook").expect("parse https url");
        let http = reqwest::Url::parse("http://example.com/webhook").expect("parse http url");
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-test", reqwest::header::HeaderValue::from_static("one"));

        let with_headers = pinned_client_key(
            &HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                default_headers: headers,
                ..Default::default()
            },
            &https,
        )
        .expect("key with headers");
        let without_headers = pinned_key_for_timeout(&https, Duration::from_secs(1));
        let different_port = pinned_key_for_timeout(&http, Duration::from_secs(1));

        assert_ne!(with_headers, without_headers);
        assert_ne!(without_headers, different_port);
    }

    #[test]
    fn pinned_client_key_normalizes_proxy_mode() {
        let url = reqwest::Url::parse("https://example.com/webhook").expect("parse url");
        let key_with_proxy_env = pinned_client_key(
            &HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                no_proxy: false,
                ..Default::default()
            },
            &url,
        )
        .expect("key with proxy env");
        let key_without_proxy_env = pinned_client_key(
            &HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                no_proxy: true,
                ..Default::default()
            },
            &url,
        )
        .expect("key without proxy env");

        assert_eq!(key_with_proxy_env, key_without_proxy_env);
    }

    #[test]
    fn resolve_override_addrs_uses_scheme_default_port_when_url_has_no_explicit_port() {
        let url = reqwest::Url::parse("http://example.com/webhook").expect("parse url");
        let addrs =
            resolve_override_addrs_for_reqwest(&url, &[SocketAddr::from(([203, 0, 113, 10], 80))]);
        assert_eq!(addrs, vec![SocketAddr::from(([203, 0, 113, 10], 0))]);
    }

    #[test]
    fn build_http_client_pinned_with_addrs_preserves_default_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0_u8; 2048];
            let read = stream.read(&mut buf).expect("read request");
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(
                request.contains("x-test-header: pinned\r\n"),
                "request should keep default headers: {request}"
            );

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("pinned"),
            );
            let options = HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                default_headers,
                ..Default::default()
            };
            let url = reqwest::Url::parse(&format!("http://example.test:{}/hook", addr.port()))
                .expect("parse url");
            let client = build_http_client_pinned_with_addrs(&options, &url, &[addr])
                .expect("build pinned client");

            let response = client.get(url).send().await.expect("send request");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn build_http_client_profile_preserves_default_headers_for_base_client() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0_u8; 2048];
            let read = stream.read(&mut buf).expect("read request");
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(
                request.contains("x-test-header: profile\r\n"),
                "request should keep default headers: {request}"
            );

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("profile"),
            );
            let profile = build_http_client_profile(&HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                default_headers,
                ..Default::default()
            })
            .expect("build http client profile");
            let url = reqwest::Url::parse(&format!("http://127.0.0.1:{}/hook", addr.port()))
                .expect("parse url");

            let response = profile
                .select_for_url(&url, false)
                .await
                .expect("select base client")
                .get(url)
                .send()
                .await
                .expect("send request");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_rejects_cross_host_redirects() {
        let redirect_listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect listener");
        let redirect_addr = redirect_listener
            .local_addr()
            .expect("redirect listener addr");
        let blocked_listener = TcpListener::bind("127.0.0.1:0").expect("bind blocked listener");
        let blocked_addr = blocked_listener
            .local_addr()
            .expect("blocked listener addr");

        let redirect_server = thread::spawn(move || {
            let (mut stream, _) = redirect_listener.accept().expect("accept redirect request");
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                blocked_addr.port()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write redirect response");
        });

        let blocked_server = thread::spawn(move || {
            blocked_listener
                .set_nonblocking(true)
                .expect("set nonblocking");
            let deadline = Instant::now() + Duration::from_millis(300);
            loop {
                match blocked_listener.accept() {
                    Ok(_) => panic!("redirect target should not be contacted"),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("accept failed: {err}"),
                }
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let url = reqwest::Url::parse(&format!(
                "http://public.example:{}/redirect",
                redirect_addr.port()
            ))
            .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[redirect_addr],
            )
            .expect("build pinned client");

            let err = client
                .get(url)
                .send()
                .await
                .expect_err("cross-host redirect should fail");
            assert!(
                err.is_redirect() || err.is_request(),
                "unexpected redirect error classification: {err}"
            );
        });

        redirect_server.join().expect("join redirect server");
        blocked_server.join().expect("join blocked server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_rejects_same_host_scheme_redirects() {
        let redirect_listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect listener");
        let redirect_addr = redirect_listener
            .local_addr()
            .expect("redirect listener addr");

        let redirect_server = thread::spawn(move || {
            let (mut stream, _) = redirect_listener.accept().expect("accept redirect request");
            let response = b"HTTP/1.1 302 Found\r\nLocation: https://public.example/secure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(response).expect("write redirect response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let url = reqwest::Url::parse(&format!(
                "http://public.example:{}/redirect",
                redirect_addr.port()
            ))
            .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[redirect_addr],
            )
            .expect("build pinned client");

            let err = client
                .get(url)
                .send()
                .await
                .expect_err("same-host scheme redirect should fail");
            assert!(
                err.is_redirect() || err.is_request(),
                "unexpected redirect error classification: {err}"
            );
        });

        redirect_server.join().expect("join redirect server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_rejects_same_host_port_redirects() {
        let redirect_listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect listener");
        let redirect_addr = redirect_listener
            .local_addr()
            .expect("redirect listener addr");
        let blocked_listener = TcpListener::bind("127.0.0.1:0").expect("bind blocked listener");
        let blocked_addr = blocked_listener
            .local_addr()
            .expect("blocked listener addr");

        let redirect_server = thread::spawn(move || {
            let (mut stream, _) = redirect_listener.accept().expect("accept redirect request");
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://public.example:{}/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                blocked_addr.port()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write redirect response");
        });

        let blocked_server = thread::spawn(move || {
            blocked_listener
                .set_nonblocking(true)
                .expect("set nonblocking");
            let deadline = Instant::now() + Duration::from_millis(300);
            loop {
                match blocked_listener.accept() {
                    Ok(_) => panic!("redirect target should not be contacted"),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("accept failed: {err}"),
                }
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let url = reqwest::Url::parse(&format!(
                "http://public.example:{}/redirect",
                redirect_addr.port()
            ))
            .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[redirect_addr],
            )
            .expect("build pinned client");

            let err = client
                .get(url)
                .send()
                .await
                .expect_err("same-host port redirect should fail");
            assert!(
                err.is_redirect() || err.is_request(),
                "unexpected redirect error classification: {err}"
            );
        });

        redirect_server.join().expect("join redirect server");
        blocked_server.join().expect("join blocked server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_allows_same_host_redirects_and_keeps_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");

        let server = thread::spawn(move || {
            for expected_path in ["/redirect", "/final"] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut buf = [0_u8; 2048];
                let read = stream.read(&mut buf).expect("read request");
                let request = String::from_utf8_lossy(&buf[..read]);

                assert!(
                    request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")),
                    "unexpected request line: {request}"
                );
                assert!(
                    request.contains("x-test-header: pinned\r\n"),
                    "request should keep default headers across same-host redirects: {request}"
                );

                let response = if expected_path == "/redirect" {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://public.example:{}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        addr.port()
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .to_string()
                };
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("pinned"),
            );
            let url =
                reqwest::Url::parse(&format!("http://public.example:{}/redirect", addr.port()))
                    .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    default_headers,
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[addr],
            )
            .expect("build pinned client");

            let response = client.get(url).send().await.expect("same-host redirect");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn select_http_client_from_profile_cleans_build_lock_on_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let url =
                reqwest::Url::parse("https://lock-cleanup.invalid/webhook").expect("parse url");
            let key = pinned_key_for_timeout(&url, Duration::ZERO);

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let profile = build_http_client_profile(&timeout_only_options(Duration::ZERO))
                .expect("build client profile");
            let err = profile
                .select_for_url(&url, true)
                .await
                .expect_err("expected dns timeout error");
            assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");

            let locks = lock_pinned_client_build_locks();
            assert!(
                !locks.contains_key(&key),
                "build lock entry should be removed after failed request"
            );
        });
    }

    #[test]
    fn select_http_client_from_profile_cleans_build_lock_on_cancel() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::from_secs(1);
            let url =
                reqwest::Url::parse("https://lock-cancel.invalid/webhook").expect("parse url");
            let key = pinned_key_for_timeout(&url, timeout);

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let semaphore_permits = dns_lookup_semaphore()
                .clone()
                .acquire_many_owned(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT as u32)
                .await
                .expect("acquire dns semaphore permits");

            let profile =
                build_http_client_profile(&timeout_only_options(timeout)).expect("build profile");
            let task = tokio::spawn({
                let profile = profile.clone();
                let url = url.clone();
                async move {
                    let _ = profile.select_for_url(&url, true).await;
                }
            });

            let inserted = tokio::time::timeout(Duration::from_millis(200), async {
                loop {
                    if lock_pinned_client_build_locks().contains_key(&key) {
                        break;
                    }
                    tokio::task::yield_now().await;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .is_ok();
            assert!(inserted, "expected build lock entry before cancellation");

            task.abort();
            let _ = task.await;
            drop(semaphore_permits);
            tokio::task::yield_now().await;

            let locks = lock_pinned_client_build_locks();
            assert!(
                !locks.contains_key(&key),
                "build lock entry should be removed after cancelled request"
            );
        });
    }

    #[test]
    fn select_http_client_from_profile_cleans_expired_cache_entry_when_refresh_fails() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::ZERO;
            let url = reqwest::Url::parse("https://expired-cache-cleanup.invalid/webhook")
                .expect("parse url");
            let key = pinned_key_for_timeout(&url, timeout);

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
                cache.insert(
                    key.clone(),
                    CachedPinnedClient {
                        client: build_http_client(Duration::from_millis(10)).expect("build client"),
                        expires_at: Instant::now() - Duration::from_secs(1),
                    },
                );
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let profile =
                build_http_client_profile(&timeout_only_options(timeout)).expect("build profile");
            let err = profile
                .select_for_url(&url, true)
                .await
                .expect_err("expected dns timeout error");
            assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");

            let cache = pinned_client_cache().read().await;
            assert!(
                !cache.contains_key(&key),
                "expired cache entry should be removed after failed refresh"
            );
        });
    }

    #[test]
    fn select_http_client_from_profile_re_resolves_dns_instead_of_reusing_cross_request_cache() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::ZERO;
            let url = reqwest::Url::parse("https://future-cache-bypass.invalid/webhook")
                .expect("parse url");
            let key = pinned_key_for_timeout(&url, timeout);

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
                cache.insert(
                    key.clone(),
                    CachedPinnedClient {
                        client: build_http_client(Duration::from_millis(10)).expect("build client"),
                        expires_at: Instant::now() + Duration::from_secs(60),
                    },
                );
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let profile =
                build_http_client_profile(&timeout_only_options(timeout)).expect("build profile");
            let err = profile
                .select_for_url(&url, true)
                .await
                .expect_err("selection should ignore seeded cross-request cache");
            assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");

            let cache = pinned_client_cache().read().await;
            assert!(
                !cache.contains_key(&key),
                "seeded cross-request cache entry should be cleared instead of being reused"
            );
        });
    }
}
