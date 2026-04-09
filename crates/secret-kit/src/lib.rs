#![forbid(unsafe_code)]

//! # 秘密管理抽象
//!
//! 本文件定义秘密解析的 trait 和接口，支持多种秘密源。
//!
//! ## 核心概念
//!
//! - **`SecretSpec`**：秘密源规范，统一的 `secret://` 格式
//! - **`SecretResolver`**：异步秘密解析 trait，typed `SecretSpec` 是公开扩展主路径，`&str` 入口只做 parse 转发
//! - **`SecretString`**：默认返回类型，`Debug` 自动脱敏，并在最后一个共享句柄 drop 时清零底层缓冲区
//! - **`SecretEnvironment`**：秘密上下文 trait，只负责提供 secret 值和缓存分区
//! - **`SecretCommandRuntime`**：CLI-backed provider 的命令运行时策略
//! - **`DefaultSecretResolver`**：默认实现
//!
//! ## 支持的秘密源
//!
//! ### 环境变量
//! ```text
//! secret://env/MY_API_KEY
//! ```
//!
//! ### 文件
//! ```text
//! secret://file?path=/etc/secrets/api_key
//! secret://file//etc/secrets/api_key
//! ```
//!
//! ### `HashiCorp` Vault
//! ```text
//! secret://vault/secret/data/my-secret?field=api_key
//! secret://vault/secret/data/my-secret?field=api_key&namespace=my-namespace
//! ```
//!
//! ### AWS Secrets Manager
//! ```text
//! secret://aws-sm/my-secret
//! secret://aws-sm/my-secret?region=us-east-1&profile=default
//! secret://aws-sm/my-secret?json_key=api_key
//! ```
//!
//! ### GCP Secret Manager
//! ```text
//! secret://gcp-sm/my-secret?project=my-project&version=latest
//! secret://gcp-sm/my-secret?project=my-project&version=1&json_key=api_key
//! ```
//!
//! ### Azure Key Vault
//! ```text
//! secret://azure-kv/my-vault/my-secret
//! secret://azure-kv/my-vault/my-secret?version=abc123
//! ```
//!
//! ## 使用场景
//!
//! ### 场景 1：从环境变量读取
//!
//! ```
//! # use secret_kit::{resolve_secret, Result, SecretCommandRuntime, SecretEnvironment};
//! # async fn example(env: &(impl SecretEnvironment + SecretCommandRuntime)) -> Result<()> {
//! let api_key = resolve_secret("secret://env/OPENAI_API_KEY", env).await?;
//! # let _ = api_key;
//! # Ok(())
//! # }
//! ```
//!
//! ### 场景 2：从文件读取
//!
//! ```
//! # use secret_kit::{resolve_secret, Result, SecretCommandRuntime, SecretEnvironment};
//! # async fn example(env: &(impl SecretEnvironment + SecretCommandRuntime)) -> Result<()> {
//! let api_key = resolve_secret(
//!     "secret://file?path=/etc/secrets/openai_key",
//!     env
//! ).await?;
//! # let _ = api_key;
//! # Ok(())
//! # }
//! ```
//!
//! ### 场景 3：从 AWS Secrets Manager 读取
//!
//! ```
//! # use secret_kit::{resolve_secret, Result, SecretCommandRuntime, SecretEnvironment};
//! # async fn example(env: &(impl SecretEnvironment + SecretCommandRuntime)) -> Result<()> {
//! let api_key = resolve_secret(
//!     "secret://aws-sm/openai-api-key?region=us-east-1",
//!     env
//! ).await?;
//! # let _ = api_key;
//! # Ok(())
//! # }
//! ```
//!
//! ### 场景 4：从 JSON 秘密中提取字段
//!
//! ```
//! # use secret_kit::{resolve_secret, Result, SecretCommandRuntime, SecretEnvironment};
//! # async fn example(env: &(impl SecretEnvironment + SecretCommandRuntime)) -> Result<()> {
//! // AWS Secrets Manager 中存储的 JSON：{"api_key": "sk-...", "org_id": "org-..."}
//! let api_key = resolve_secret(
//!     "secret://aws-sm/openai-credentials?json_key=api_key",
//!     env
//! ).await?;
//! # let _ = api_key;
//! # Ok(())
//! # }
//! ```
//!
//! ## 设计注意事项
//!
//! - **异步设计**：秘密解析可能涉及网络调用（云服务），因此使用异步 trait
//! - **CLI 工具依赖**：云服务秘密源依赖相应的 CLI 工具（aws, gcloud, az）
//! - **环境变量注入**：某些秘密源支持通过查询参数注入环境变量（如 AWS profile）
//! - **JSON 字段提取**：支持从 JSON 秘密中提取特定字段
//! - **错误处理**：所有秘密解析错误都返回 `SecretError`，支持国际化错误消息
//! - **空值语义**：解析器保留空字符串，调用方自行决定空秘密是否可接受
//! - **秘密所有权**：默认 API 返回 `SecretString`；`SecretString::into_inner` 只在独占时交出底层字符串，避免偷偷复制明文
//! - **明文导出责任**：`SecretString::expose_secret` 和 `SecretString::into_owned` 会把明文交给调用方；一旦调用方复制或长期持有这些值，zeroize 保障就只剩当前容器，不会替外部副本擦屁股
//! - **内建 CLI 发现**：内建 provider 默认只信任 ambient allowlist 里指向系统级目录的 `PATH` 快照项来找 `vault`/`aws`/`gcloud`/`az`，并把传给这些 CLI 的 ambient `PATH` 同步裁剪到同一可信目录集合；workspace、用户目录、`/tmp` 之类的绝对路径不会因为“是绝对路径”就自动变成可信搜索入口，显式 command env 也不能重写这个搜索，生产环境优先提供绝对路径 override
//! - **命令失败诊断**：CLI provider 失败时只返回退出状态、`stderr` 字节数和粗粒度 `stderr_hint`；不会回显原始 `stderr` 明文，且 `stderr_hint` 仅供排障，不是稳定协议
//! - **缓存分区**：`CachingSecretResolver` 只会为显式提供非空 `secret_cache_partition()` 的环境复用缓存；runtime-sensitive secret 还需要稳定的 `SecretCommandRuntime::secret_cache_partition()`。相同分区必须表示相同的非秘密解析上下文，空分区会被当成未分区处理
//! - **文件路径**：`secret://file` 只接受绝对路径，避免把当前工作目录偷偷变成配置输入
//! - **文件轮换**：允许目录内的符号链接轮换（如 Kubernetes projected volumes），拒绝越出父目录树的链接
//! - **进程清理**：Unix 下会清理整个进程组；Linux 额外校验 leader 身份以降低 PID/PGID 复用误杀风险；Windows 下优先使用 Job Object，失败时退回 runtime 层的 best-effort 树清理
//!
//! ## 反模式
//!
//! - ❌ 不要硬编码秘密值
//! - ❌ 不要在日志中打印秘密值
//! - ❌ 不要假设秘密源总是可用（总是处理错误）
//! - ❌ 不要混合多个秘密源（使用统一的 `secret://` 格式）

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

pub use runtime::{
    AmbientSecretCommandRuntime, SecretCommandRuntime, SecretEnvironment, SecretResolutionContext,
};
pub use types::{Result, SecretError};
pub use value::SecretString;
use value::{SecretBytes, read_limited, secret_string_from_bytes};

pub type SecretResolveFuture<'a> = Pin<Box<dyn Future<Output = Result<SecretString>> + Send + 'a>>;

macro_rules! invalid_response {
    ($code:literal $(,)?) => {
        SecretError::InvalidSpec(structured_text_kit::structured_text!($code))
    };
    ($code:literal, $($rest:tt)*) => {
        SecretError::InvalidSpec(structured_text_kit::structured_text!($code, $($rest)*))
    };
}

macro_rules! secret_command_error {
    ($code:literal $(,)?) => {
        SecretError::Command(structured_text_kit::structured_text!($code))
    };
    ($code:literal, $($rest:tt)*) => {
        SecretError::Command(structured_text_kit::structured_text!($code, $($rest)*))
    };
}

macro_rules! secret_io_error {
    ($code:literal, $source:expr $(,)?) => {
        SecretError::io(structured_text_kit::structured_text!($code), $source)
    };
    ($code:literal, $source:expr, $($rest:tt)*) => {
        SecretError::io(
            structured_text_kit::structured_text!($code, $($rest)*),
            $source,
        )
    };
}

macro_rules! secret_json_error {
    ($code:literal, $source:expr $(,)?) => {
        SecretError::json(structured_text_kit::structured_text!($code), $source)
    };
    ($code:literal, $source:expr, $($rest:tt)*) => {
        SecretError::json(
            structured_text_kit::structured_text!($code, $($rest)*),
            $source,
        )
    };
}

pub trait SecretResolver: Send + Sync {
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a crate::spec::SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        let spec = spec.to_string();
        Box::pin(async move { self.resolve_secret(&spec, context).await })
    }

    fn resolve_secret<'a>(
        &'a self,
        spec: &'a str,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let parsed = crate::spec::SecretSpec::parse(spec)?;
            self.resolve_secret_spec(&parsed, context).await
        })
    }
}

impl<R> SecretResolver for Box<R>
where
    R: SecretResolver + ?Sized,
{
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a crate::spec::SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        self.as_ref().resolve_secret_spec(spec, context)
    }

    fn resolve_secret<'a>(
        &'a self,
        spec: &'a str,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        self.as_ref().resolve_secret(spec, context)
    }
}

impl<R> SecretResolver for std::sync::Arc<R>
where
    R: SecretResolver + ?Sized,
{
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a crate::spec::SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        self.as_ref().resolve_secret_spec(spec, context)
    }

    fn resolve_secret<'a>(
        &'a self,
        spec: &'a str,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        self.as_ref().resolve_secret(spec, context)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretCachePartitioning {
    Environment,
    EnvironmentAndCommandRuntime,
}

pub struct PreparedSecretResolution<P> {
    prepared: P,
    cache_scope: Option<String>,
    cache_partitioning: SecretCachePartitioning,
}

impl<P> PreparedSecretResolution<P> {
    #[must_use]
    pub fn uncached(prepared: P) -> Self {
        Self {
            prepared,
            cache_scope: None,
            cache_partitioning: SecretCachePartitioning::Environment,
        }
    }

    #[must_use]
    pub fn cached(prepared: P, cache_scope: impl Into<String>) -> Self {
        Self::cached_with_partitioning(prepared, cache_scope, SecretCachePartitioning::Environment)
    }

    #[must_use]
    pub fn cached_with_partitioning(
        prepared: P,
        cache_scope: impl Into<String>,
        cache_partitioning: SecretCachePartitioning,
    ) -> Self {
        Self {
            prepared,
            cache_scope: normalize_secret_cache_component(cache_scope.into()),
            cache_partitioning,
        }
    }

    #[must_use]
    pub fn cache_scope(&self) -> Option<&str> {
        self.cache_scope.as_deref()
    }

    fn cache_key(&self, context: SecretResolutionContext<'_>) -> Option<SecretCacheKey> {
        self.cache_scope.as_ref().and_then(|scope| {
            SecretCacheKey::for_context(scope.clone(), self.cache_partitioning, context)
        })
    }

    /// Extract the prepared resolution payload.
    pub fn into_prepared(self) -> P {
        self.prepared
    }
}

pub trait CacheAwareSecretResolver: SecretResolver {
    type Prepared: Send;

    /// Optionally provide a cache-scope hint before preparing the resolution.
    ///
    /// When this returns `Some(scope)` for a cacheable secret, `prepare_secret_resolution`
    /// should later return the same scope via [`PreparedSecretResolution::cached`]. Mismatched
    /// hints are treated as cache misses by the caching decorator.
    ///
    /// Hints should be cheap and side-effect free. If determining the scope would require
    /// blocking I/O, reading secret contents, or otherwise doing real resolution work, return
    /// `None` and let `prepare_secret_resolution` handle it instead.
    fn lookup_secret_cache_scope(
        &self,
        _spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    /// Optionally provide a cache-scope hint for a parsed secret spec before preparing the
    /// resolution.
    fn lookup_secret_cache_scope_for_spec(
        &self,
        spec: &crate::spec::SecretSpec,
        context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        self.lookup_secret_cache_scope(&spec.to_string(), context)
    }

    /// Declare how a cache-scope hint should be partitioned.
    ///
    /// Returning `None` disables the hint-only fast path and falls back to the prepared-resolution
    /// path, which is the fail-closed default for resolvers that have not audited whether their
    /// cacheable secrets also depend on [`SecretCommandRuntime`].
    fn lookup_secret_cache_partitioning(
        &self,
        _spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        None
    }

    /// Declare how a parsed cache-scope hint should be partitioned.
    fn lookup_secret_cache_partitioning_for_spec(
        &self,
        spec: &crate::spec::SecretSpec,
        context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        self.lookup_secret_cache_partitioning(&spec.to_string(), context)
    }

    fn prepare_secret_resolution(
        &self,
        spec: &str,
        context: SecretResolutionContext<'_>,
    ) -> impl Future<Output = Result<PreparedSecretResolution<Self::Prepared>>> + Send {
        async move {
            let parsed = crate::spec::SecretSpec::parse(spec)?;
            self.prepare_secret_spec_resolution(&parsed, context).await
        }
    }

    fn prepare_secret_spec_resolution(
        &self,
        spec: &crate::spec::SecretSpec,
        context: SecretResolutionContext<'_>,
    ) -> impl Future<Output = Result<PreparedSecretResolution<Self::Prepared>>> + Send {
        let spec = spec.to_string();
        async move { self.prepare_secret_resolution(&spec, context).await }
    }

    fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> impl Future<Output = Result<SecretString>> + Send;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultSecretResolver;

impl SecretResolver for DefaultSecretResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a crate::spec::SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move { resolve_secret_spec_in_context(spec, context).await })
    }

    fn resolve_secret<'a>(
        &'a self,
        spec: &'a str,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move { resolve_secret_in_context(spec, context).await })
    }
}

pub struct DefaultPreparedSecret {
    spec: SecretSpec,
}

impl CacheAwareSecretResolver for DefaultSecretResolver {
    type Prepared = DefaultPreparedSecret;

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &crate::spec::SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(crate::spec::prepare_default_secret_spec_resolution(
            spec.clone(),
        ))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        resolve_prepared_default_secret(prepared, context).await
    }
}

/// Optional resolver decorator that caches successful secret resolutions for a bounded TTL.
///
/// The wrapped resolver decides which specs are cacheable by providing a cache scope. When that
/// scope can be derived from the raw spec, the hint can cheaply identify candidate cache hits, but
/// the prepared cache key remains authoritative so mismatched hints fail closed instead of
/// reusing another secret's entry. Cache entries are always partitioned by
/// [`SecretEnvironment::secret_cache_partition`], and runtime-sensitive secrets can additionally
/// opt into [`SecretCommandRuntime::secret_cache_partition`]. Missing required partitions disable
/// cache reuse so secrets from disjoint environments or command runtimes cannot bleed into one
/// another through a shared resolver.
pub struct CachingSecretResolver<R> {
    inner: R,
    state: Mutex<SecretCacheState>,
    ttl: Duration,
    max_entries: usize,
}

#[derive(Default)]
struct SecretCacheState {
    entries: BTreeMap<SecretCacheKey, SecretCacheEntry>,
    lru: VecDeque<SecretCacheKey>,
    inflight: BTreeMap<SecretCacheKey, broadcast::Sender<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SecretCacheKey {
    env_partition: String,
    runtime_partition: Option<String>,
    scope: String,
}

struct SecretCacheEntry {
    inserted_at: Instant,
    value: SecretString,
}

enum SecretCacheLookup {
    Hit(SecretString),
    Wait(broadcast::Receiver<()>),
    Leader(broadcast::Sender<()>),
}

struct SecretCacheFillGuard<'a, R> {
    resolver: &'a CachingSecretResolver<R>,
    key: Option<SecretCacheKey>,
    waiter_done: broadcast::Sender<()>,
    value: Option<SecretString>,
}

impl<R> CachingSecretResolver<R> {
    /// Create a caching resolver with an explicit TTL.
    pub fn new(inner: R, ttl: Duration) -> Self {
        Self {
            inner,
            state: Mutex::new(SecretCacheState::default()),
            ttl,
            max_entries: 256,
        }
    }

    /// Limit the number of retained cache entries. Older entries are evicted first.
    #[must_use]
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    /// Borrow the wrapped resolver.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Consume the decorator and return the wrapped resolver.
    pub fn into_inner(self) -> R {
        self.inner
    }

    fn hinted_cache_key(
        &self,
        spec: &crate::spec::SecretSpec,
        context: SecretResolutionContext<'_>,
    ) -> Result<Option<SecretCacheKey>>
    where
        R: CacheAwareSecretResolver + Send + Sync,
    {
        Ok(self
            .inner
            .lookup_secret_cache_scope_for_spec(spec, context)?
            .and_then(|scope| {
                self.inner
                    .lookup_secret_cache_partitioning_for_spec(spec, context)
                    .and_then(|partitioning| {
                        SecretCacheKey::for_context(scope, partitioning, context)
                    })
            }))
    }

    fn lookup_cache(&self, key: &SecretCacheKey) -> SecretCacheLookup {
        let mut state = lock_cache_state(&self.state);
        state.prune_expired(self.ttl);
        if let Some(entry) = state.entries.get(key) {
            let value = entry.value.clone();
            state.touch_key(key);
            return SecretCacheLookup::Hit(value);
        }
        if let Some(waiter_done) = state.inflight.get(key) {
            return SecretCacheLookup::Wait(waiter_done.subscribe());
        }

        let (waiter_done, _) = broadcast::channel(1);
        state.inflight.insert(key.clone(), waiter_done.clone());
        SecretCacheLookup::Leader(waiter_done)
    }

    fn cached_value(&self, key: &SecretCacheKey) -> Option<SecretString> {
        let mut state = lock_cache_state(&self.state);
        state.prune_expired(self.ttl);
        let value = state.entries.get(key).map(|entry| entry.value.clone())?;
        state.touch_key(key);
        Some(value)
    }

    async fn resolve_with_fill(
        &self,
        prepared: R::Prepared,
        context: SecretResolutionContext<'_>,
        mut fill: SecretCacheFillGuard<'_, R>,
    ) -> Result<SecretString>
    where
        R: CacheAwareSecretResolver + Send + Sync,
    {
        let result = self.inner.resolve_prepared_secret(prepared, context).await;
        if let Ok(value) = &result {
            fill.store_value(value);
        }
        result
    }
}

impl<R> SecretResolver for CachingSecretResolver<R>
where
    R: CacheAwareSecretResolver + Send + Sync,
{
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a crate::spec::SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            loop {
                let hinted_key = self.hinted_cache_key(spec, context)?;
                let hinted_value = hinted_key.as_ref().and_then(|key| self.cached_value(key));

                let prepared = self
                    .inner
                    .prepare_secret_spec_resolution(spec, context)
                    .await?;
                let prepared_key = prepared.cache_key(context);

                let Some(key) = prepared_key else {
                    return self
                        .inner
                        .resolve_prepared_secret(prepared.into_prepared(), context)
                        .await;
                };

                if hinted_key
                    .as_ref()
                    .is_some_and(|hinted_key| hinted_key == &key)
                    && let Some(value) = hinted_value
                {
                    return Ok(value);
                }

                match self.lookup_cache(&key) {
                    SecretCacheLookup::Hit(value) => return Ok(value),
                    SecretCacheLookup::Wait(mut waiter_done) => {
                        let _ = waiter_done.recv().await;
                    }
                    SecretCacheLookup::Leader(waiter_done) => {
                        let fill = SecretCacheFillGuard::new(self, key.clone(), waiter_done);
                        return self
                            .resolve_with_fill(prepared.into_prepared(), context, fill)
                            .await;
                    }
                }
            }
        })
    }
}

impl<'a, R> SecretCacheFillGuard<'a, R> {
    fn new(
        resolver: &'a CachingSecretResolver<R>,
        key: SecretCacheKey,
        waiter_done: broadcast::Sender<()>,
    ) -> Self {
        Self {
            resolver,
            key: Some(key),
            waiter_done,
            value: None,
        }
    }

    fn store_value(&mut self, value: &SecretString) {
        self.value = Some(value.clone());
    }
}

impl<R> Drop for SecretCacheFillGuard<'_, R> {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };

        let mut state = lock_cache_state(&self.resolver.state);
        state.prune_expired(self.resolver.ttl);
        state.inflight.remove(&key);
        if let Some(value) = self.value.take() {
            state.insert(key, value, self.resolver.max_entries);
        }
        drop(state);

        let _ = self.waiter_done.send(());
    }
}

fn lock_cache_state(state: &Mutex<SecretCacheState>) -> MutexGuard<'_, SecretCacheState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SecretCacheState {
    fn prune_expired(&mut self, ttl: Duration) {
        let expired = self
            .entries
            .iter()
            .filter_map(|(key, entry)| (entry.inserted_at.elapsed() >= ttl).then_some(key.clone()))
            .collect::<Vec<_>>();
        for key in expired {
            self.remove_key(&key);
        }
    }

    fn insert(&mut self, key: SecretCacheKey, value: SecretString, max_entries: usize) {
        if self
            .entries
            .insert(
                key.clone(),
                SecretCacheEntry {
                    inserted_at: Instant::now(),
                    value,
                },
            )
            .is_some()
        {
            self.remove_key_from_lru(&key);
        }
        self.lru.push_back(key);
        self.evict_to_capacity(max_entries);
    }

    fn touch_key(&mut self, key: &SecretCacheKey) {
        if self.lru.back().is_some_and(|existing| existing == key) {
            return;
        }
        self.remove_key_from_lru(key);
        self.lru.push_back(key.clone());
    }

    fn evict_to_capacity(&mut self, max_entries: usize) {
        while self.entries.len() > max_entries {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn remove_key(&mut self, key: &SecretCacheKey) {
        self.entries.remove(key);
        self.remove_key_from_lru(key);
    }

    fn remove_key_from_lru(&mut self, key: &SecretCacheKey) {
        if let Some(index) = self.lru.iter().position(|existing| existing == key) {
            self.lru.remove(index);
        }
    }
}

mod command;
mod file;
mod json;
pub mod runtime;
pub mod spec;
mod types;
mod value;

use spec::{resolve_secret_in_context, resolve_secret_spec_in_context};

pub use spec::{
    SecretSpec, prepare_default_secret_spec_resolution, resolve_prepared_default_secret,
    resolve_secret, resolve_secret_spec, resolve_secret_spec_with_runtime,
    resolve_secret_with_runtime,
};

impl SecretCacheKey {
    fn for_context(
        scope: String,
        partitioning: SecretCachePartitioning,
        context: SecretResolutionContext<'_>,
    ) -> Option<Self> {
        let env_partition = normalize_secret_cache_component(
            context.environment().secret_cache_partition()?.into_owned(),
        )?;
        let scope = normalize_secret_cache_component(scope)?;
        let runtime_partition = match partitioning {
            SecretCachePartitioning::Environment => None,
            SecretCachePartitioning::EnvironmentAndCommandRuntime => {
                Some(normalize_secret_cache_component(
                    context
                        .command_runtime()
                        .secret_cache_partition()?
                        .into_owned(),
                )?)
            }
        };
        Some(Self {
            env_partition,
            runtime_partition,
            scope,
        })
    }
}

fn normalize_secret_cache_component(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(windows)]
fn os_env_var_name_matches(candidate: &OsStr, expected: &OsStr) -> bool {
    match (candidate.to_str(), expected.to_str()) {
        (Some(candidate), Some(expected)) => candidate.eq_ignore_ascii_case(expected),
        _ => candidate == expected,
    }
}

#[cfg(not(windows))]
fn os_env_var_name_matches(candidate: &OsStr, expected: &OsStr) -> bool {
    candidate == expected
}

const DEFAULT_SECRET_COMMAND_TIMEOUT_SECS: u64 = 15;
const MAX_SECRET_COMMAND_TIMEOUT_SECS: u64 = 300;
const MAX_SECRET_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_SECRET_FILE_BYTES: usize = 64 * 1024;
const MAX_SECRET_FILE_SYMLINK_DEPTH: usize = 16;
const SECRET_COMMAND_TIMEOUT_MS_ENV: &str = "SECRET_COMMAND_TIMEOUT_MS";
const SECRET_COMMAND_TIMEOUT_SECS_ENV: &str = "SECRET_COMMAND_TIMEOUT_SECS";

#[cfg(test)]
mod tests;
