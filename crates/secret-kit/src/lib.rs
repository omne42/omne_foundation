#![forbid(unsafe_code)]

//! # 秘密管理抽象
//!
//! 本文件定义秘密解析的 trait 和接口，支持多种秘密源。
//!
//! ## 核心概念
//!
//! - **`SecretSpec`**：秘密源规范，统一的 `secret://` 格式
//! - **`SecretResolver`**：异步秘密解析 trait
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
//! ```ignore
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
//! ```ignore
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
//! ```ignore
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
//! ```ignore
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
//! - **内建 CLI 发现**：内建 provider 默认只信任 ambient allowlist 里的绝对 `PATH` 快照项来找 `vault`/`aws`/`gcloud`/`az`；显式 command env 不能重写这个搜索，生产环境优先提供绝对路径 override
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

use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::error::Error as StdError;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};
use tokio::sync::broadcast;
use zeroize::Zeroize;

use structured_text_kit::{CatalogTextRef, StructuredText, structured_text};

#[derive(Debug)]
pub enum SecretError {
    Io {
        text: StructuredText,
        source: std::io::Error,
    },
    Json {
        text: StructuredText,
        source: serde_json::Error,
    },
    Lookup(StructuredText),
    InvalidSpec(StructuredText),
    Command(StructuredText),
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// Heap-backed secret text that redacts itself in `Debug` output and zeroizes its shared buffer
/// when the last handle drops.
#[derive(Clone, Default)]
pub struct SecretString(Arc<SecretText>);

#[derive(Default)]
struct SecretText(String);

impl SecretText {
    fn into_inner(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::new(SecretText(value.into())))
    }

    /// Borrow the plaintext secret.
    ///
    /// This returns a normal `&str`. If the caller copies it into another `String`, logs it, or
    /// stores it elsewhere, that external copy is outside `SecretString`'s zeroization contract.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.0.as_str()
    }

    /// Extract the owned secret string without cloning.
    ///
    /// This only succeeds when the current handle uniquely owns the underlying buffer.
    pub fn into_inner(self) -> std::result::Result<String, Self> {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(shared) => Err(Self(shared)),
        }
    }

    /// Consume the secret and return owned plaintext.
    ///
    /// This reuses the underlying allocation when the current handle is unique and clones only
    /// when the secret buffer is shared, such as after cache hits.
    ///
    /// The returned `String` is ordinary owned plaintext. Once it leaves `SecretString`, the
    /// caller is responsible for its lifetime and any further scrubbing.
    #[must_use]
    pub fn into_owned(self) -> String {
        match self.into_inner() {
            Ok(value) => value,
            Err(shared) => shared.expose_secret().to_owned(),
        }
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl Drop for SecretText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Raw secret bytes that zeroize their current buffer on drop.
#[derive(Default)]
struct SecretBytes(Vec<u8>);

impl SecretBytes {
    fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn into_inner(mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

impl AsRef<[u8]> for SecretBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes(<redacted>, len={})", self.0.len())
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

struct ZeroizingByteBuffer<const N: usize>([u8; N]);

impl<const N: usize> ZeroizingByteBuffer<N> {
    fn new() -> Self {
        Self([0u8; N])
    }
}

impl<const N: usize> AsRef<[u8]> for ZeroizingByteBuffer<N> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> AsMut<[u8]> for ZeroizingByteBuffer<N> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl<const N: usize> Drop for ZeroizingByteBuffer<N> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl SecretError {
    fn io_retry_advice(source: &std::io::Error) -> ErrorRetryAdvice {
        match source.kind() {
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::Unsupported => ErrorRetryAdvice::DoNotRetry,
            _ => ErrorRetryAdvice::Retryable,
        }
    }

    fn command_retry_advice(text: &StructuredText) -> ErrorRetryAdvice {
        match text.as_catalog().map(CatalogTextRef::code) {
            Some("error_detail.secret.command_timeout")
            | Some("error_detail.secret.command_spawn_failed")
            | Some("error_detail.secret.command_output_read_failed") => ErrorRetryAdvice::Retryable,
            _ => ErrorRetryAdvice::DoNotRetry,
        }
    }

    #[must_use]
    pub fn structured_text(&self) -> &StructuredText {
        match self {
            Self::Io { text, .. }
            | Self::Json { text, .. }
            | Self::Lookup(text)
            | Self::InvalidSpec(text)
            | Self::Command(text) => text,
        }
    }

    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::Io { .. } => {
                ErrorCode::try_new("secret.io").expect("literal error code should validate")
            }
            Self::Json { .. } => {
                ErrorCode::try_new("secret.json").expect("literal error code should validate")
            }
            Self::Lookup(_) => {
                ErrorCode::try_new("secret.lookup").expect("literal error code should validate")
            }
            Self::InvalidSpec(_) => ErrorCode::try_new("secret.invalid_spec")
                .expect("literal error code should validate"),
            Self::Command(_) => {
                ErrorCode::try_new("secret.command").expect("literal error code should validate")
            }
        }
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        match self {
            Self::Io { .. } => ErrorCategory::ExternalDependency,
            Self::Json { .. } => ErrorCategory::InvalidInput,
            Self::Lookup(_) => ErrorCategory::NotFound,
            Self::InvalidSpec(_) => ErrorCategory::InvalidInput,
            Self::Command(_) => ErrorCategory::ExternalDependency,
        }
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        match self {
            Self::Io { source, .. } => Self::io_retry_advice(source),
            Self::Json { .. } => ErrorRetryAdvice::DoNotRetry,
            Self::Lookup(_) => ErrorRetryAdvice::DoNotRetry,
            Self::InvalidSpec(_) => ErrorRetryAdvice::DoNotRetry,
            Self::Command(text) => Self::command_retry_advice(text),
        }
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        ErrorRecord::new(self.error_code(), self.structured_text().clone())
            .with_category(self.error_category())
            .with_retry_advice(self.retry_advice())
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        let category = self.error_category();
        let retry_advice = self.retry_advice();
        match self {
            Self::Io { text, source } => ErrorRecord::new(
                ErrorCode::try_new("secret.io").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice)
            .with_source(source),
            Self::Json { text, source } => ErrorRecord::new(
                ErrorCode::try_new("secret.json").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice)
            .with_source(source),
            Self::Lookup(text) => ErrorRecord::new(
                ErrorCode::try_new("secret.lookup").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice),
            Self::InvalidSpec(text) => ErrorRecord::new(
                ErrorCode::try_new("secret.invalid_spec")
                    .expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice),
            Self::Command(text) => ErrorRecord::new(
                ErrorCode::try_new("secret.command").expect("literal error code should validate"),
                text,
            )
            .with_category(category)
            .with_retry_advice(retry_advice),
        }
    }

    fn io(text: StructuredText, source: std::io::Error) -> Self {
        Self::Io { text, source }
    }

    fn json(text: StructuredText, source: serde_json::Error) -> Self {
        Self::Json { text, source }
    }

    fn lookup(text: StructuredText) -> Self {
        Self::Lookup(text)
    }
}

impl Display for SecretError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { text, source } => write!(
                f,
                "secret io error: {}: {source}",
                text.diagnostic_display()
            ),
            Self::Json { text, source } => {
                write!(
                    f,
                    "secret json error: {}: {source}",
                    text.diagnostic_display()
                )
            }
            Self::Lookup(text) => {
                write!(f, "secret lookup error: {}", text.diagnostic_display())
            }
            Self::InvalidSpec(text) => {
                write!(f, "invalid secret spec: {}", text.diagnostic_display())
            }
            Self::Command(text) => {
                write!(f, "secret command error: {}", text.diagnostic_display())
            }
        }
    }
}

impl StdError for SecretError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Lookup(_) | Self::InvalidSpec(_) | Self::Command(_) => None,
        }
    }
}

impl From<std::io::Error> for SecretError {
    fn from(source: std::io::Error) -> Self {
        let error = source.to_string();
        Self::io(
            structured_text!("error_detail.secret.io_error", "error" => error),
            source,
        )
    }
}

impl From<serde_json::Error> for SecretError {
    fn from(source: serde_json::Error) -> Self {
        let error = source.to_string();
        Self::json(
            structured_text!("error_detail.secret.json_error", "error" => error),
            source,
        )
    }
}

impl From<SecretError> for ErrorRecord {
    fn from(error: SecretError) -> Self {
        error.into_error_record()
    }
}

macro_rules! invalid_response {
    ($code:literal $(,)?) => {
        SecretError::InvalidSpec(structured_text!($code))
    };
    ($code:literal, $($rest:tt)*) => {
        SecretError::InvalidSpec(structured_text!($code, $($rest)*))
    };
}

macro_rules! secret_command_error {
    ($code:literal $(,)?) => {
        SecretError::Command(structured_text!($code))
    };
    ($code:literal, $($rest:tt)*) => {
        SecretError::Command(structured_text!($code, $($rest)*))
    };
}

macro_rules! secret_io_error {
    ($code:literal, $source:expr $(,)?) => {
        SecretError::io(structured_text!($code), $source)
    };
    ($code:literal, $source:expr, $($rest:tt)*) => {
        SecretError::io(structured_text!($code, $($rest)*), $source)
    };
}

macro_rules! secret_json_error {
    ($code:literal, $source:expr $(,)?) => {
        SecretError::json(structured_text!($code), $source)
    };
    ($code:literal, $source:expr, $($rest:tt)*) => {
        SecretError::json(structured_text!($code, $($rest)*), $source)
    };
}

pub trait SecretEnvironment: Send + Sync {
    fn get_secret(&self, key: &str) -> Option<SecretString>;

    /// Stable partition key used by [`CachingSecretResolver`] to isolate cached secrets.
    ///
    /// The value should be stable for the lifetime of the environment instance and should reflect
    /// the non-secret context that can affect secret resolution, such as a deployment name,
    /// account identifier, or configuration profile.
    /// It must never contain secret material or request-unique noise. A partition derived from a
    /// secret value defeats the point of cache isolation, and a partition that changes on every
    /// request silently disables reuse.
    /// Different resolution contexts must return different partitions. Reusing a partition means
    /// the caller is asserting that cacheable secrets resolve identically across those instances.
    ///
    /// Returning `None` disables cache reuse for this environment. Empty partitions are treated
    /// the same way. This is the safe default when no stable, non-secret partition key exists.
    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        None
    }
}

/// Command-execution policy used by CLI-backed secret providers.
///
/// This is intentionally separate from [`SecretEnvironment`]. Secret lookup is domain state;
/// child-process environment shaping and binary resolution are runtime policy.
///
/// Async secret resolution for CLI-backed providers enforces command timeouts via `tokio::time`,
/// so callers need a Tokio runtime with the time driver enabled.
pub trait SecretCommandRuntime: Send + Sync {
    /// Stable partition key used by [`CachingSecretResolver`] for runtime-sensitive secrets.
    ///
    /// Cacheable resolutions that depend on command discovery, explicit command environment, or
    /// other runtime policy should include this partition in their cache key. Returning `None`
    /// disables cache reuse for runtime-sensitive secrets, which is the safe default when no
    /// stable, non-secret runtime identity exists.
    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Targeted command-environment lookup for control-plane settings and runtime overrides.
    ///
    /// This does not automatically populate spawned child processes. Use `command_env_pairs` or
    /// `command_env_os_pairs` for explicit child environment injection.
    /// Secret command timeout tuning also does not consult this hook; if you want a resolver-local
    /// timeout, put the `SECRET_COMMAND_TIMEOUT_*` variables into the explicit command
    /// snapshot instead of relying on ambient process state.
    fn get_command_env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    /// Targeted command-environment lookup for control-plane settings and runtime overrides.
    ///
    /// Look up a command-environment value while sharing the same explicit snapshot used for child
    /// process injection when possible.
    fn get_command_env_os(&self, key: &OsStr) -> Option<OsString> {
        self.command_env_os_pairs()
            .find_map(|(candidate, value)| {
                os_env_var_name_matches(candidate.as_os_str(), key).then_some(value)
            })
            .or_else(|| {
                key.to_str()
                    .and_then(|key| self.get_command_env(key).map(OsString::from))
            })
    }

    /// Explicit child-process environment snapshot.
    ///
    /// Values returned here are injected into spawned commands after the ambient allowlist.
    fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
        Box::new(std::iter::empty())
    }

    fn command_env_os_pairs(&self) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
        Box::new(
            self.command_env_pairs()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
    }

    fn ambient_command_env_pairs(
        &self,
        program: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + '_> {
        command::filtered_ambient_command_env_pairs(program)
    }

    fn ambient_command_env_os_pairs(
        &self,
        program: &str,
    ) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
        command::filtered_ambient_command_env_os_pairs(program)
    }

    /// Resolve the executable used for a secret CLI command.
    ///
    /// Built-in providers only accept absolute override paths whose basename still matches the
    /// original provider binary (for example `/tmp/vault` for `vault`). Without an override they
    /// resolve the program from absolute entries in the ambient allowlisted `PATH` snapshot, not
    /// from explicit `command_env_pairs` injection.
    fn resolve_command_program(&self, _program: &str) -> Option<String> {
        None
    }
}

#[derive(Clone, Copy)]
pub struct SecretResolutionContext<'a> {
    environment: &'a dyn SecretEnvironment,
    command_runtime: &'a dyn SecretCommandRuntime,
}

impl<'a> SecretResolutionContext<'a> {
    #[must_use]
    pub fn new(
        environment: &'a dyn SecretEnvironment,
        command_runtime: &'a dyn SecretCommandRuntime,
    ) -> Self {
        Self {
            environment,
            command_runtime,
        }
    }

    #[must_use]
    pub fn ambient(environment: &'a dyn SecretEnvironment) -> Self {
        Self::new(environment, &AMBIENT_SECRET_COMMAND_RUNTIME)
    }

    #[must_use]
    pub fn environment(self) -> &'a dyn SecretEnvironment {
        self.environment
    }

    #[must_use]
    pub fn command_runtime(self) -> &'a dyn SecretCommandRuntime {
        self.command_runtime
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AmbientSecretCommandRuntime;

impl SecretCommandRuntime for AmbientSecretCommandRuntime {
    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("ambient"))
    }
}

static AMBIENT_SECRET_COMMAND_RUNTIME: AmbientSecretCommandRuntime = AmbientSecretCommandRuntime;

pub trait SecretResolver: Send + Sync {
    fn resolve_secret(
        &self,
        spec: &str,
        context: SecretResolutionContext<'_>,
    ) -> impl Future<Output = Result<SecretString>> + Send;
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

    fn prepare_secret_resolution(
        &self,
        spec: &str,
        context: SecretResolutionContext<'_>,
    ) -> impl Future<Output = Result<PreparedSecretResolution<Self::Prepared>>> + Send;

    fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> impl Future<Output = Result<SecretString>> + Send;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultSecretResolver;

impl SecretResolver for DefaultSecretResolver {
    async fn resolve_secret(
        &self,
        spec: &str,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        resolve_secret_in_context(spec, context).await
    }
}

pub struct DefaultPreparedSecret {
    spec: SecretSpec,
}

impl CacheAwareSecretResolver for DefaultSecretResolver {
    type Prepared = DefaultPreparedSecret;

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        prepare_default_secret_resolution(spec).await
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
/// scope can be derived from the raw spec, cache hits avoid running the expensive resolution path
/// entirely. Cache entries are always partitioned by
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
    async fn resolve_secret(
        &self,
        spec: &str,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        loop {
            if let Some(key) = self
                .inner
                .lookup_secret_cache_scope(spec, context)?
                .and_then(|scope| {
                    self.inner
                        .lookup_secret_cache_partitioning(spec, context)
                        .and_then(|partitioning| {
                            SecretCacheKey::for_context(scope, partitioning, context)
                        })
                })
                && let Some(value) = self.cached_value(&key)
            {
                return Ok(value);
            }

            let prepared = self.inner.prepare_secret_resolution(spec, context).await?;
            let prepared_key = prepared.cache_key(context);

            let Some(key) = prepared_key else {
                return self
                    .inner
                    .resolve_prepared_secret(prepared.into_prepared(), context)
                    .await;
            };

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
mod spec;

use spec::{
    prepare_default_secret_resolution, resolve_prepared_default_secret, resolve_secret_in_context,
};

pub use spec::{SecretSpec, resolve_secret, resolve_secret_with_runtime};

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

async fn read_limited<R>(mut reader: R, max_bytes: usize) -> std::io::Result<(SecretBytes, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut out = SecretBytes::with_capacity(max_bytes);
    let mut buf = ZeroizingByteBuffer::<4096>::new();
    let mut truncated = false;
    loop {
        let remaining = max_bytes.saturating_sub(out.len());
        let read_len = buf.as_ref().len().min(remaining.saturating_add(1).max(1));
        let n = reader.read(&mut buf.as_mut()[..read_len]).await?;
        if n == 0 {
            break;
        }

        if n > remaining {
            out.extend_from_slice(&buf.as_ref()[..remaining]);
            truncated = true;
            break;
        }

        out.extend_from_slice(&buf.as_ref()[..n]);
    }
    Ok((out, truncated))
}

fn secret_string_from_bytes(
    bytes: SecretBytes,
    invalid_utf8_error: impl FnOnce(std::str::Utf8Error) -> SecretError,
) -> Result<SecretString> {
    match String::from_utf8(bytes.into_inner()) {
        Ok(value) => Ok(SecretString::from(value)),
        Err(err) => {
            let utf8_error = err.utf8_error();
            let mut bytes = err.into_bytes();
            bytes.zeroize();
            Err(invalid_utf8_error(utf8_error))
        }
    }
}

#[cfg(test)]
mod tests;
