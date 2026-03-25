use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as TokioMutex, RwLock, Semaphore};

use crate::public_ip::validate_public_addrs;

const DEFAULT_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT: usize = 32;
const DEFAULT_PINNED_CLIENT_TTL: Duration = Duration::from_secs(60);
const DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PinnedClientKey {
    host: String,
    timeout: Duration,
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

fn dns_lookup_timeout_message() -> &'static str {
    DNS_LOOKUP_TIMEOUT_MESSAGE
        .get_or_init(|| format!("dns lookup timeout (capped at {DEFAULT_DNS_LOOKUP_TIMEOUT:?})"))
        .as_str()
}

fn pinned_client_cache() -> &'static RwLock<HashMap<PinnedClientKey, CachedPinnedClient>> {
    PINNED_CLIENT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
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
            .min_by(|(lhs_key, lhs_val), (rhs_key, rhs_val)| {
                (lhs_val.expires_at, lhs_key.host.as_str(), lhs_key.timeout).cmp(&(
                    rhs_val.expires_at,
                    rhs_key.host.as_str(),
                    rhs_key.timeout,
                ))
            })
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&key);
    }
}

fn build_http_client_builder(options: &HttpClientOptions) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .redirect(if options.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .default_headers(options.default_headers.clone());

    if options.no_proxy {
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
            tokio::net::lookup_host((host, 443)),
        )
        .await
        .map_err(|_| anyhow::anyhow!(dns_lookup_timeout_message()))?
        .map_err(|err| anyhow::anyhow!("dns lookup failed: {err}"))?
    };

    validate_public_addrs(lookup)
}

async fn build_http_client_pinned_async(
    timeout: Duration,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;

    let addrs = resolve_url_to_public_addrs_async(url, timeout).await?;

    build_http_client_builder(&HttpClientOptions {
        timeout: Some(timeout),
        ..Default::default()
    })
    .resolve_to_addrs(host, &addrs)
    .build()
    .map_err(|err| anyhow::anyhow!("build reqwest client: {err}").into())
}

pub async fn select_http_client(
    base_client: &reqwest::Client,
    timeout: Duration,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> crate::Result<reqwest::Client> {
    if !enforce_public_ip {
        return Ok(base_client.clone());
    }

    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;
    let key = PinnedClientKey {
        host: host.to_string(),
        timeout,
    };

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
            Ok(client)
        } else {
            let client = build_http_client_pinned_async(timeout, url).await?;
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
        }
    }
    .await;

    drop(key_lock);
    cleanup_pinned_client_build_lock_entry(&key);
    build_lock_cleanup.disarm();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let host = "example.com".to_string();
        let lhs = PinnedClientKey {
            host: host.clone(),
            timeout: Duration::from_micros(500),
        };
        let rhs = PinnedClientKey {
            host,
            timeout: Duration::from_micros(900),
        };
        assert_ne!(lhs, rhs);
    }

    #[test]
    fn select_http_client_cleans_build_lock_on_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let url =
                reqwest::Url::parse("https://lock-cleanup.invalid/webhook").expect("parse url");
            let key = PinnedClientKey {
                host: "lock-cleanup.invalid".to_string(),
                timeout: Duration::ZERO,
            };

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let client = build_http_client(Duration::from_millis(10)).expect("build client");
            let err = select_http_client(&client, Duration::ZERO, &url, true)
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
    fn select_http_client_cleans_build_lock_on_cancel() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::from_secs(1);
            let url =
                reqwest::Url::parse("https://lock-cancel.invalid/webhook").expect("parse url");
            let key = PinnedClientKey {
                host: "lock-cancel.invalid".to_string(),
                timeout,
            };

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

            let client = build_http_client(timeout).expect("build client");
            let task = tokio::spawn({
                let client = client.clone();
                let url = url.clone();
                async move {
                    let _ = select_http_client(&client, timeout, &url, true).await;
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
    fn select_http_client_cleans_expired_cache_entry_when_refresh_fails() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::ZERO;
            let url = reqwest::Url::parse("https://expired-cache-cleanup.invalid/webhook")
                .expect("parse url");
            let key = PinnedClientKey {
                host: "expired-cache-cleanup.invalid".to_string(),
                timeout,
            };

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

            let client = build_http_client(Duration::from_millis(10)).expect("build client");
            let err = select_http_client(&client, timeout, &url, true)
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
}
