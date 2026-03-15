//! # 秘密管理抽象
//!
//! 本文件定义秘密解析的 trait 和接口，支持多种秘密源。
//!
//! ## 核心概念
//!
//! - **`SecretSpec`**：秘密源规范，统一的 `secret://` 格式
//! - **`SecretResolver`**：异步秘密解析 trait
//! - **`SecretEnvironment`**：秘密环境变量访问 trait
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
//! ### HashiCorp Vault
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
//! let resolver = DefaultSecretResolver;
//! let env = /* 实现 SecretEnvironment */;
//! let api_key = resolver.resolve_secret_string("secret://env/OPENAI_API_KEY", &env).await?;
//! ```
//!
//! ### 场景 2：从文件读取
//!
//! ```ignore
//! let api_key = resolver.resolve_secret_string(
//!     "secret://file?path=/etc/secrets/openai_key",
//!     &env
//! ).await?;
//! ```
//!
//! ### 场景 3：从 AWS Secrets Manager 读取
//!
//! ```ignore
//! let api_key = resolver.resolve_secret_string(
//!     "secret://aws-sm/openai-api-key?region=us-east-1",
//!     &env
//! ).await?;
//! ```
//!
//! ### 场景 4：从 JSON 秘密中提取字段
//!
//! ```ignore
//! // AWS Secrets Manager 中存储的 JSON：{"api_key": "sk-...", "org_id": "org-..."}
//! let api_key = resolver.resolve_secret_string(
//!     "secret://aws-sm/openai-credentials?json_key=api_key",
//!     &env
//! ).await?;
//! ```
//!
//! ## 设计注意事项
//!
//! - **异步设计**：秘密解析可能涉及网络调用（云服务），因此使用异步 trait
//! - **CLI 工具依赖**：云服务秘密源依赖相应的 CLI 工具（aws, gcloud, az）
//! - **环境变量注入**：某些秘密源支持通过查询参数注入环境变量（如 AWS profile）
//! - **JSON 字段提取**：支持从 JSON 秘密中提取特定字段
//! - **错误处理**：所有秘密解析错误都返回 `SecretError`，支持国际化错误消息
//!
//! ## 反模式
//!
//! - ❌ 不要硬编码秘密值
//! - ❌ 不要在日志中打印秘密值
//! - ❌ 不要假设秘密源总是可用（总是处理错误）
//! - ❌ 不要混合多个秘密源（使用统一的 `secret://` 格式）

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

use async_trait::async_trait;

use error::{StructuredMessage, structured_message};

#[derive(Debug)]
pub enum SecretError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidSpec(StructuredMessage),
    AuthCommand(StructuredMessage),
}

pub type Result<T> = std::result::Result<T, SecretError>;

impl Display for SecretError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => Display::fmt(err, f),
            Self::Json(err) => Display::fmt(err, f),
            Self::InvalidSpec(message) => write!(f, "invalid secret spec: {message}"),
            Self::AuthCommand(message) => write!(f, "secret auth command error: {message}"),
        }
    }
}

impl StdError for SecretError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::InvalidSpec(_) | Self::AuthCommand(_) => None,
        }
    }
}

impl From<std::io::Error> for SecretError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for SecretError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

macro_rules! invalid_response {
    ($code:expr $(,)?) => {
        SecretError::InvalidSpec(structured_message!($code))
    };
    ($code:expr, $($rest:tt)*) => {
        SecretError::InvalidSpec(structured_message!($code, $($rest)*))
    };
}

macro_rules! auth_command_error {
    ($code:expr $(,)?) => {
        SecretError::AuthCommand(structured_message!($code))
    };
    ($code:expr, $($rest:tt)*) => {
        SecretError::AuthCommand(structured_message!($code, $($rest)*))
    };
}

fn secret_scheme_missing() -> SecretError {
    invalid_response!("error_detail.secret.scheme_missing")
}

fn secret_env_key_required() -> SecretError {
    invalid_response!("error_detail.secret.env_key_required")
}

fn secret_file_path_required() -> SecretError {
    invalid_response!("error_detail.secret.file_path_required")
}

fn secret_vault_path_required() -> SecretError {
    invalid_response!("error_detail.secret.vault_path_required")
}

fn secret_aws_secret_id_required() -> SecretError {
    invalid_response!("error_detail.secret.aws_secret_id_required")
}

fn secret_gcp_secret_required() -> SecretError {
    invalid_response!("error_detail.secret.gcp_secret_required")
}

fn secret_azure_vault_name_required() -> SecretError {
    invalid_response!("error_detail.secret.azure_vault_name_required")
}

fn unsupported_secret_provider(provider: &str) -> SecretError {
    invalid_response!(
        "error_detail.secret.unsupported_provider",
        "provider" => provider
    )
}

fn missing_env_var(key: &str) -> SecretError {
    auth_command_error!("error_detail.auth.missing_env_var", "key" => key)
}

fn secret_file_empty(path: &str) -> SecretError {
    invalid_response!("error_detail.secret.file_empty", "path" => path)
}

fn secret_not_resolvable() -> SecretError {
    invalid_response!("error_detail.secret.not_resolvable")
}

fn command_spawn_failed(program: &str, error: impl Display) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_spawn_failed",
        "program" => program,
        "error" => error.to_string()
    )
}

fn command_stdout_not_captured(program: &str) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_stdout_not_captured",
        "program" => program
    )
}

fn command_stderr_not_captured(program: &str) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_stderr_not_captured",
        "program" => program
    )
}

fn command_wait_failed(program: &str, error: impl Display) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_wait_failed",
        "program" => program,
        "error" => error.to_string()
    )
}

fn command_timeout(program: &str, timeout_ms: u128) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_timeout",
        "program" => program,
        "timeout_ms" => timeout_ms.to_string()
    )
}

fn command_reader_join_failed(stream: &str, error: impl Display) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_reader_join_failed",
        "stream" => stream,
        "error" => error.to_string()
    )
}

fn command_stdout_too_large(program: &str, max_bytes: usize) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_stdout_too_large",
        "program" => program,
        "max_bytes" => max_bytes.to_string()
    )
}

fn command_stderr_too_large(program: &str, max_bytes: usize) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_stderr_too_large",
        "program" => program,
        "max_bytes" => max_bytes.to_string()
    )
}

fn command_failed_status(program: &str, status: &str) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_failed_status",
        "program" => program,
        "status" => status
    )
}

fn command_empty_stdout(program: &str) -> SecretError {
    auth_command_error!(
        "error_detail.auth.command_empty_stdout",
        "program" => program
    )
}

fn secret_json_missing_key(key: &str) -> SecretError {
    invalid_response!("error_detail.secret.json_missing_key", "key" => key)
}

pub trait SecretEnvironment: Send + Sync {
    fn get_secret_env(&self, key: &str) -> Option<String>;
}

#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve_secret_string(
        &self,
        spec: &str,
        env: &dyn SecretEnvironment,
    ) -> Result<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultSecretResolver;

#[async_trait]
impl SecretResolver for DefaultSecretResolver {
    async fn resolve_secret_string(
        &self,
        spec: &str,
        env: &dyn SecretEnvironment,
    ) -> Result<String> {
        resolve_secret_string(spec, env).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSpec {
    Env {
        key: String,
    },
    File {
        path: String,
    },
    VaultCli {
        path: String,
        field: String,
        namespace: Option<String>,
    },
    AwsSecretsManagerCli {
        secret_id: String,
        region: Option<String>,
        profile: Option<String>,
        json_key: Option<String>,
    },
    GcpSecretManagerCli {
        secret: String,
        project: Option<String>,
        version: String,
        json_key: Option<String>,
    },
    AzureKeyVaultCli {
        vault: String,
        name: String,
        version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub json_key: Option<String>,
}

impl SecretSpec {
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        let rest = input
            .strip_prefix("secret://")
            .ok_or_else(secret_scheme_missing)?;

        let (head, query) = rest.split_once('?').unwrap_or((rest, ""));
        let query = parse_query(query);

        let (provider, tail) = head
            .split_once('/')
            .map(|(provider, tail)| (provider, Some(tail)))
            .unwrap_or((head, None));
        let provider = provider.trim();

        match provider {
            "env" => {
                let key = tail.unwrap_or_default().trim();
                if key.is_empty() {
                    return Err(secret_env_key_required());
                }
                Ok(Self::Env {
                    key: key.to_string(),
                })
            }
            "file" => {
                let path = query
                    .get("path")
                    .cloned()
                    .or_else(|| tail.map(|v| v.to_string()))
                    .unwrap_or_default();
                let path = path.trim();
                if path.is_empty() {
                    return Err(secret_file_path_required());
                }
                Ok(Self::File {
                    path: path.to_string(),
                })
            }
            "vault" => {
                let path = tail.unwrap_or_default().trim();
                if path.is_empty() {
                    return Err(secret_vault_path_required());
                }
                let field = query
                    .get("field")
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "token".to_string());
                let namespace = query
                    .get("namespace")
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty());
                Ok(Self::VaultCli {
                    path: path.to_string(),
                    field,
                    namespace,
                })
            }
            "aws-sm" | "aws-secrets-manager" => {
                let secret_id = tail.unwrap_or_default().trim();
                if secret_id.is_empty() {
                    return Err(secret_aws_secret_id_required());
                }
                Ok(Self::AwsSecretsManagerCli {
                    secret_id: secret_id.to_string(),
                    region: query
                        .get("region")
                        .cloned()
                        .filter(|v| !v.trim().is_empty()),
                    profile: query
                        .get("profile")
                        .cloned()
                        .filter(|v| !v.trim().is_empty()),
                    json_key: query
                        .get("json_key")
                        .cloned()
                        .filter(|v| !v.trim().is_empty()),
                })
            }
            "gcp-sm" | "gcp-secret-manager" => {
                let secret = tail.unwrap_or_default().trim();
                if secret.is_empty() {
                    return Err(secret_gcp_secret_required());
                }
                Ok(Self::GcpSecretManagerCli {
                    secret: secret.to_string(),
                    project: query
                        .get("project")
                        .cloned()
                        .filter(|v| !v.trim().is_empty()),
                    version: query
                        .get("version")
                        .cloned()
                        .filter(|v| !v.trim().is_empty())
                        .unwrap_or_else(|| "latest".to_string()),
                    json_key: query
                        .get("json_key")
                        .cloned()
                        .filter(|v| !v.trim().is_empty()),
                })
            }
            "azure-kv" | "azure-key-vault" => {
                let tail = tail.unwrap_or_default();
                let (vault, name) = tail
                    .split_once('/')
                    .ok_or_else(secret_azure_vault_name_required)?;
                let vault = vault.trim();
                let name = name.trim();
                if vault.is_empty() || name.is_empty() {
                    return Err(secret_azure_vault_name_required());
                }
                Ok(Self::AzureKeyVaultCli {
                    vault: vault.to_string(),
                    name: name.to_string(),
                    version: query
                        .get("version")
                        .cloned()
                        .filter(|v| !v.trim().is_empty()),
                })
            }
            other => Err(unsupported_secret_provider(other)),
        }
    }

    pub fn build_command(&self) -> Option<SecretCommand> {
        match self {
            SecretSpec::Env { .. } | SecretSpec::File { .. } => None,
            SecretSpec::VaultCli {
                path,
                field,
                namespace,
            } => {
                let mut env = BTreeMap::new();
                if let Some(namespace) = namespace.as_deref() {
                    env.insert("VAULT_NAMESPACE".to_string(), namespace.to_string());
                }
                Some(SecretCommand {
                    program: "vault".to_string(),
                    args: vec![
                        "kv".to_string(),
                        "get".to_string(),
                        format!("-field={field}"),
                        path.clone(),
                    ],
                    env,
                    json_key: None,
                })
            }
            SecretSpec::AwsSecretsManagerCli {
                secret_id,
                region,
                profile,
                json_key,
            } => {
                let mut args = vec![
                    "secretsmanager".to_string(),
                    "get-secret-value".to_string(),
                    "--secret-id".to_string(),
                    secret_id.clone(),
                    "--query".to_string(),
                    "SecretString".to_string(),
                    "--output".to_string(),
                    "text".to_string(),
                ];
                if let Some(region) = region.as_deref() {
                    args.push("--region".to_string());
                    args.push(region.to_string());
                }
                if let Some(profile) = profile.as_deref() {
                    args.push("--profile".to_string());
                    args.push(profile.to_string());
                }
                Some(SecretCommand {
                    program: "aws".to_string(),
                    args,
                    env: BTreeMap::new(),
                    json_key: json_key.clone(),
                })
            }
            SecretSpec::GcpSecretManagerCli {
                secret,
                project,
                version,
                json_key,
            } => {
                let mut args = vec![
                    "secrets".to_string(),
                    "versions".to_string(),
                    "access".to_string(),
                    version.clone(),
                    "--secret".to_string(),
                    secret.clone(),
                ];
                if let Some(project) = project.as_deref() {
                    args.push("--project".to_string());
                    args.push(project.to_string());
                }
                Some(SecretCommand {
                    program: "gcloud".to_string(),
                    args,
                    env: BTreeMap::new(),
                    json_key: json_key.clone(),
                })
            }
            SecretSpec::AzureKeyVaultCli {
                vault,
                name,
                version,
            } => {
                let mut args = vec![
                    "keyvault".to_string(),
                    "secret".to_string(),
                    "show".to_string(),
                    "--vault-name".to_string(),
                    vault.clone(),
                    "--name".to_string(),
                    name.clone(),
                    "--query".to_string(),
                    "value".to_string(),
                    "-o".to_string(),
                    "tsv".to_string(),
                ];
                if let Some(version) = version.as_deref() {
                    args.push("--version".to_string());
                    args.push(version.to_string());
                }
                Some(SecretCommand {
                    program: "az".to_string(),
                    args,
                    env: BTreeMap::new(),
                    json_key: None,
                })
            }
        }
    }

    pub async fn resolve(&self, env: &dyn SecretEnvironment) -> Result<String> {
        match self {
            SecretSpec::Env { key } => env
                .get_secret_env(key)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing_env_var(key)),
            SecretSpec::File { path } => {
                let contents = tokio::fs::read_to_string(path).await?;
                let value = contents.trim().to_string();
                if value.is_empty() {
                    return Err(secret_file_empty(path));
                }
                Ok(value)
            }
            other => {
                let cmd = other.build_command().ok_or_else(secret_not_resolvable)?;
                let value = run_secret_command(&cmd, env).await?;
                if let Some(json_key) = cmd.json_key.as_deref() {
                    let extracted = extract_json_key(&value, json_key)?;
                    return Ok(extracted);
                }
                Ok(value)
            }
        }
    }
}

const DEFAULT_SECRET_COMMAND_TIMEOUT_SECS: u64 = 15;
const MAX_SECRET_COMMAND_TIMEOUT_SECS: u64 = 300;
const SECRET_COMMAND_TIMEOUT_MS_ENV: &str = "DITTO_SECRET_COMMAND_TIMEOUT_MS";
const SECRET_COMMAND_TIMEOUT_SECS_ENV: &str = "DITTO_SECRET_COMMAND_TIMEOUT_SECS";

fn positive_env_u64(env: &dyn SecretEnvironment, key: &str) -> Option<u64> {
    env.get_secret_env(key)
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
}

// Keep both env vars for backward compatibility; the millisecond variant wins when both are set.
fn secret_command_timeout(env: &dyn SecretEnvironment) -> Duration {
    let timeout_ms = positive_env_u64(env, SECRET_COMMAND_TIMEOUT_MS_ENV)
        .or_else(|| {
            positive_env_u64(env, SECRET_COMMAND_TIMEOUT_SECS_ENV)
                .map(|secs| secs.saturating_mul(1_000))
        })
        .unwrap_or(DEFAULT_SECRET_COMMAND_TIMEOUT_SECS.saturating_mul(1_000))
        .min(MAX_SECRET_COMMAND_TIMEOUT_SECS.saturating_mul(1_000));
    Duration::from_millis(timeout_ms)
}

async fn run_secret_command(cmd: &SecretCommand, env: &dyn SecretEnvironment) -> Result<String> {
    let timeout = secret_command_timeout(env);

    let mut command = tokio::process::Command::new(cmd.program.as_str());
    command.args(&cmd.args);
    for (key, value) in &cmd.env {
        command.env(key, value);
    }

    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    // Keep spawned CLIs from surviving task cancellation or early returns.
    command.kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|err| command_spawn_failed(cmd.program.as_str(), err))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| command_stdout_not_captured(cmd.program.as_str()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| command_stderr_not_captured(cmd.program.as_str()))?;

    const MAX_SECRET_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

    let stdout_task = tokio::spawn(read_limited(stdout, MAX_SECRET_COMMAND_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(read_limited(stderr, MAX_SECRET_COMMAND_OUTPUT_BYTES));

    let timeout_error = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => {
            let status = status.map_err(|err| command_wait_failed(cmd.program.as_str(), err))?;
            Ok(status)
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(command_timeout(cmd.program.as_str(), timeout.as_millis()))
        }
    };

    let status = match timeout_error {
        Ok(status) => status,
        Err(err) => {
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(err);
        }
    };

    let (stdout, stdout_truncated) = stdout_task
        .await
        .map_err(|err| command_reader_join_failed("stdout", err))??;
    let (_stderr, stderr_truncated) = stderr_task
        .await
        .map_err(|err| command_reader_join_failed("stderr", err))??;

    if stdout_truncated {
        return Err(command_stdout_too_large(
            cmd.program.as_str(),
            MAX_SECRET_COMMAND_OUTPUT_BYTES,
        ));
    }
    if stderr_truncated {
        return Err(command_stderr_too_large(
            cmd.program.as_str(),
            MAX_SECRET_COMMAND_OUTPUT_BYTES,
        ));
    }

    if !status.success() {
        // Stderr may contain secret material; never attach it to the structured error payload.
        return Err(command_failed_status(
            cmd.program.as_str(),
            &status.to_string(),
        ));
    }

    let stdout = String::from_utf8_lossy(&stdout);
    let value = stdout.trim().to_string();
    if value.is_empty() {
        return Err(command_empty_stdout(cmd.program.as_str()));
    }
    Ok(value)
}

async fn read_limited<R>(mut reader: R, max_bytes: usize) -> Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut out = Vec::<u8>::new();
    let mut buf = [0u8; 4096];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }

        if truncated {
            continue;
        }

        let remaining = max_bytes.saturating_sub(out.len());
        if remaining == 0 {
            truncated = true;
            continue;
        }

        if n > remaining {
            out.extend_from_slice(&buf[..remaining]);
            truncated = true;
            continue;
        }

        out.extend_from_slice(&buf[..n]);
    }
    Ok((out, truncated))
}

pub async fn resolve_secret_string(spec: &str, env: &dyn SecretEnvironment) -> Result<String> {
    let spec = SecretSpec::parse(spec)?;
    spec.resolve(env).await
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::<String, String>::new();
    for pair in query.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        out.insert(key.to_string(), value.trim().to_string());
    }
    out
}

fn extract_json_key(json: &str, key: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let mut cursor = &value;
    let mut resolved_path = String::new();
    for part in key.split('.').map(str::trim).filter(|p| !p.is_empty()) {
        if !resolved_path.is_empty() {
            resolved_path.push('.');
        }
        resolved_path.push_str(part);
        cursor = cursor
            .get(part)
            .ok_or_else(|| secret_json_missing_key(resolved_path.as_str()))?;
    }
    match cursor {
        serde_json::Value::Null => Err(secret_json_missing_key(if resolved_path.is_empty() {
            key
        } else {
            resolved_path.as_str()
        })),
        serde_json::Value::String(value) => Ok(value.clone()),
        other => serde_json::to_string(other).map_err(Into::into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestEnv {
        vars: BTreeMap<String, String>,
    }

    impl SecretEnvironment for TestEnv {
        fn get_secret_env(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    #[tokio::test]
    async fn resolves_env_secret() -> Result<()> {
        let env = TestEnv {
            vars: BTreeMap::from([("TEST_SECRET".to_string(), "ok".to_string())]),
        };
        let value = resolve_secret_string("secret://env/TEST_SECRET", &env).await?;
        assert_eq!(value, "ok");
        Ok(())
    }

    #[tokio::test]
    async fn resolves_file_secret() -> Result<()> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secret.txt");
        tokio::fs::write(&path, "  hello  \n").await?;
        let env = TestEnv::default();
        let value = resolve_secret_string(
            &format!("secret://file?path={}", path.to_string_lossy()),
            &env,
        )
        .await?;
        assert_eq!(value, "hello");
        Ok(())
    }

    #[test]
    fn parses_command_specs() -> Result<()> {
        let spec = SecretSpec::parse(
            "secret://aws-sm/mysecret?region=us-east-1&profile=dev&json_key=token",
        )?;
        let cmd = spec.build_command().expect("command");
        assert_eq!(cmd.program, "aws");
        assert!(cmd.args.iter().any(|arg| arg == "secretsmanager"));
        assert_eq!(cmd.json_key.as_deref(), Some("token"));

        let spec = SecretSpec::parse("secret://azure-kv/myvault/mysecret")?;
        let cmd = spec.build_command().expect("command");
        assert_eq!(cmd.program, "az");

        let spec = SecretSpec::parse("secret://vault/secret/openai?field=api_key&namespace=team")?;
        let cmd = spec.build_command().expect("command");
        assert_eq!(cmd.program, "vault");
        assert_eq!(
            cmd.env.get("VAULT_NAMESPACE").map(String::as_str),
            Some("team")
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn secret_command_runner_returns_stdout() -> Result<()> {
        let env = TestEnv::default();
        let cmd = SecretCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "echo ok".to_string()],
            env: BTreeMap::new(),
            json_key: None,
        };
        let value = run_secret_command(&cmd, &env).await?;
        assert_eq!(value, "ok");
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn secret_command_runner_times_out() -> Result<()> {
        let env = TestEnv {
            vars: BTreeMap::from([(
                "DITTO_SECRET_COMMAND_TIMEOUT_MS".to_string(),
                "10".to_string(),
            )]),
        };
        let cmd = SecretCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 1; echo ok".to_string()],
            env: BTreeMap::new(),
            json_key: None,
        };
        let err = run_secret_command(&cmd, &env).await.unwrap_err();
        let SecretError::AuthCommand(message) = err else {
            panic!("expected auth command error");
        };
        assert_eq!(message.code(), "error_detail.auth.command_timeout");
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn secret_command_runner_discards_stderr_from_errors() -> Result<()> {
        let env = TestEnv::default();
        let cmd = SecretCommand {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo leaked-secret >&2; exit 1".to_string(),
            ],
            env: BTreeMap::new(),
            json_key: None,
        };
        let err = run_secret_command(&cmd, &env).await.unwrap_err();
        let rendered = err.to_string();
        let SecretError::AuthCommand(message) = &err else {
            panic!("expected auth command error");
        };
        assert_eq!(message.code(), "error_detail.auth.command_failed_status");
        assert!(message.args().iter().all(|arg| arg.name() != "stderr"));
        assert!(!rendered.contains("leaked-secret"));
        assert!(!rendered.contains("stderr="));
        Ok(())
    }

    #[test]
    fn secret_command_timeout_prefers_ms_env_when_both_are_set() {
        let env = TestEnv {
            vars: BTreeMap::from([
                (SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(), "250".to_string()),
                (
                    SECRET_COMMAND_TIMEOUT_SECS_ENV.to_string(),
                    "30".to_string(),
                ),
            ]),
        };

        assert_eq!(secret_command_timeout(&env), Duration::from_millis(250));
    }

    #[test]
    fn secret_command_timeout_clamps_large_ms_values() {
        let env = TestEnv {
            vars: BTreeMap::from([(
                SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
                "999999".to_string(),
            )]),
        };

        assert_eq!(
            secret_command_timeout(&env),
            Duration::from_secs(MAX_SECRET_COMMAND_TIMEOUT_SECS)
        );
    }

    #[test]
    fn extract_json_key_reports_missing_nested_path() {
        let err = extract_json_key(r#"{"outer":{"present":"ok"}}"#, "outer.missing").unwrap_err();
        let SecretError::InvalidSpec(message) = err else {
            panic!("expected invalid spec error");
        };

        assert_eq!(message.code(), "error_detail.secret.json_missing_key");
        let key = message
            .args()
            .iter()
            .find(|arg| arg.name() == "key")
            .and_then(|arg| arg.text());
        assert_eq!(key, Some("outer.missing"));
    }

    #[test]
    fn extract_json_key_treats_null_as_missing() {
        let err = extract_json_key(r#"{"outer":{"token":null}}"#, "outer.token").unwrap_err();
        let SecretError::InvalidSpec(message) = err else {
            panic!("expected invalid spec error");
        };

        assert_eq!(message.code(), "error_detail.secret.json_missing_key");
        let key = message
            .args()
            .iter()
            .find(|arg| arg.name() == "key")
            .and_then(|arg| arg.text());
        assert_eq!(key, Some("outer.token"));
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[tokio::test]
    async fn secret_command_runner_cancellation_kills_child_process() -> Result<()> {
        fn process_terminated_or_zombie(pid: u32) -> bool {
            let status_path = format!("/proc/{pid}/status");
            match std::fs::read_to_string(status_path) {
                Ok(status) => status
                    .lines()
                    .find(|line| line.starts_with("State:"))
                    .map(|line| line.contains("\tZ") || line.contains(" zombie"))
                    .unwrap_or(false),
                Err(err) => err.kind() == std::io::ErrorKind::NotFound,
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("secret-command.pid");
        let script = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
        let cmd = SecretCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), script],
            env: BTreeMap::new(),
            json_key: None,
        };
        let env = TestEnv::default();

        let handle = tokio::spawn(async move {
            let _ = run_secret_command(&cmd, &env).await;
        });

        let mut pid: Option<u32> = None;
        for _ in 0..100 {
            if let Ok(raw) = tokio::fs::read_to_string(&pid_file).await {
                let parsed = raw.trim().parse::<u32>().ok();
                if parsed.is_some() {
                    pid = parsed;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = pid.expect("pid file should be written");

        handle.abort();
        let _ = handle.await;

        let mut gone = false;
        for _ in 0..300 {
            if process_terminated_or_zombie(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            gone,
            "secret command child process should be killed on cancellation"
        );
        Ok(())
    }
}
