use std::collections::BTreeMap;
use std::path::Path;

use structured_text_kit::structured_text;

use crate::command::run_secret_command;
use crate::file::read_secret_file;
use crate::json::{extract_json_key_secret, validate_json_key_path};
use crate::{
    DefaultPreparedSecret, PreparedSecretResolution, Result, SecretCommandRuntime,
    SecretEnvironment, SecretError, SecretResolutionContext, SecretString,
};

#[derive(Clone, PartialEq, Eq)]
pub enum SecretSpec {
    Env {
        key: String,
    },
    File {
        path: String,
    },
    Vault {
        path: String,
        field: String,
        namespace: Option<String>,
    },
    AwsSecretsManager {
        secret_id: String,
        region: Option<String>,
        profile: Option<String>,
        json_key: Option<String>,
    },
    GcpSecretManager {
        secret: String,
        project: Option<String>,
        version: String,
        json_key: Option<String>,
    },
    AzureKeyVault {
        vault: String,
        name: String,
        version: Option<String>,
    },
}

impl std::fmt::Debug for SecretSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Env { .. } => f
                .debug_struct("SecretSpec::Env")
                .field("key", &"<redacted>")
                .finish(),
            Self::File { .. } => f
                .debug_struct("SecretSpec::File")
                .field("path", &"<redacted>")
                .finish(),
            Self::Vault { namespace, .. } => f
                .debug_struct("SecretSpec::Vault")
                .field("path", &"<redacted>")
                .field("field", &"<redacted>")
                .field("has_namespace", &namespace.is_some())
                .finish(),
            Self::AwsSecretsManager {
                region,
                profile,
                json_key,
                ..
            } => f
                .debug_struct("SecretSpec::AwsSecretsManager")
                .field("secret_id", &"<redacted>")
                .field("has_region", &region.is_some())
                .field("has_profile", &profile.is_some())
                .field("has_json_key", &json_key.is_some())
                .finish(),
            Self::GcpSecretManager {
                project, json_key, ..
            } => f
                .debug_struct("SecretSpec::GcpSecretManager")
                .field("secret", &"<redacted>")
                .field("has_project", &project.is_some())
                .field("version", &"<redacted>")
                .field("has_json_key", &json_key.is_some())
                .finish(),
            Self::AzureKeyVault { version, .. } => f
                .debug_struct("SecretSpec::AzureKeyVault")
                .field("vault", &"<redacted>")
                .field("name", &"<redacted>")
                .field("has_version", &version.is_some())
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SecretCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub json_key: Option<String>,
}

impl std::fmt::Debug for SecretCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_keys = self.env.keys().collect::<Vec<_>>();
        f.debug_struct("SecretCommand")
            .field("program", &self.program)
            .field("arg_count", &self.args.len())
            .field("env_keys", &env_keys)
            .field("json_key", &self.json_key)
            .finish()
    }
}

impl SecretSpec {
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        let rest = input
            .strip_prefix("secret://")
            .ok_or_else(|| invalid_response!("error_detail.secret.scheme_missing"))?;

        let (head, query) = rest.split_once('?').unwrap_or((rest, ""));
        let query = parse_query(query)?;
        let (provider, tail) = split_provider_and_tail(head);
        parse_secret_spec(provider.trim(), tail, &query)
    }
}

pub(crate) async fn prepare_default_secret_resolution(
    spec: &str,
) -> Result<PreparedSecretResolution<DefaultPreparedSecret>> {
    let parsed = SecretSpec::parse(spec)?;
    let prepared = DefaultPreparedSecret { spec: parsed };
    Ok(PreparedSecretResolution::uncached(prepared))
}

pub(crate) async fn resolve_prepared_default_secret(
    prepared: DefaultPreparedSecret,
    context: SecretResolutionContext<'_>,
) -> Result<SecretString> {
    resolve_secret_spec(&prepared.spec, context).await
}

pub(crate) async fn resolve_secret_in_context(
    spec: &str,
    context: SecretResolutionContext<'_>,
) -> Result<SecretString> {
    let prepared = prepare_default_secret_resolution(spec).await?;
    resolve_prepared_default_secret(prepared.into_prepared(), context).await
}

pub async fn resolve_secret<E>(spec: &str, env: &E) -> Result<SecretString>
where
    E: SecretEnvironment + SecretCommandRuntime,
{
    resolve_secret_in_context(spec, SecretResolutionContext::new(env, env)).await
}

pub async fn resolve_secret_with_runtime(
    spec: &str,
    environment: &dyn SecretEnvironment,
    command_runtime: &dyn SecretCommandRuntime,
) -> Result<SecretString> {
    resolve_secret_in_context(
        spec,
        SecretResolutionContext::new(environment, command_runtime),
    )
    .await
}

pub(crate) async fn resolve_secret_spec(
    spec: &SecretSpec,
    context: SecretResolutionContext<'_>,
) -> Result<SecretString> {
    match spec {
        SecretSpec::Env { key } => context.environment().get_secret(key).ok_or_else(|| {
            SecretError::lookup(structured_text!(
                "error_detail.secret.missing_env_var",
                "key" => key
            ))
        }),
        SecretSpec::File { path } => read_secret_file(Path::new(path)).await,
        other => {
            let cmd = build_secret_command(other)
                .ok_or_else(|| invalid_response!("error_detail.secret.not_resolvable"))?;
            let value = run_secret_command(&cmd, context.command_runtime()).await?;
            if let Some(json_key) = cmd.json_key.as_deref() {
                return extract_json_key_secret(value, json_key);
            }
            Ok(value)
        }
    }
}

type QueryParameters = BTreeMap<String, String>;

fn split_provider_and_tail(head: &str) -> (&str, Option<&str>) {
    head.split_once('/')
        .map_or((head, None), |(provider, tail)| (provider, Some(tail)))
}

fn parse_secret_spec(
    provider: &str,
    tail: Option<&str>,
    query: &QueryParameters,
) -> Result<SecretSpec> {
    match provider {
        "env" => parse_env_spec(tail, query),
        "file" => parse_file_spec(tail, query),
        "vault" => parse_vault_spec(tail, query),
        "aws-sm" => parse_aws_sm_spec(tail, query),
        "gcp-sm" => parse_gcp_sm_spec(tail, query),
        "azure-kv" => parse_azure_kv_spec(tail, query),
        other => Err(invalid_response!(
            "error_detail.secret.unsupported_provider",
            "provider" => other
        )),
    }
}

fn parse_env_spec(tail: Option<&str>, query: &QueryParameters) -> Result<SecretSpec> {
    ensure_allowed_query_parameters("env", query, &[])?;
    let key = decode_required_tail_component(tail, || {
        invalid_response!("error_detail.secret.env_key_required")
    })?;
    Ok(SecretSpec::Env { key })
}

fn parse_file_spec(tail: Option<&str>, query: &QueryParameters) -> Result<SecretSpec> {
    ensure_allowed_query_parameters("file", query, &["path"])?;
    let path = match (query.get("path"), tail) {
        (Some(_), Some(_)) => {
            return Err(invalid_response!("error_detail.secret.file_path_conflict"));
        }
        (Some(path), None) => path.clone(),
        (None, Some(path)) => decode_spec_component(path)?,
        (None, None) => String::new(),
    };
    if path.is_empty() {
        return Err(invalid_response!("error_detail.secret.file_path_required"));
    }
    if !Path::new(&path).is_absolute() {
        return Err(invalid_response!(
            "error_detail.secret.file_path_must_be_absolute",
            "path" => path
        ));
    }
    Ok(SecretSpec::File { path })
}

fn parse_vault_spec(tail: Option<&str>, query: &QueryParameters) -> Result<SecretSpec> {
    ensure_allowed_query_parameters("vault", query, &["field", "namespace"])?;
    let path = decode_required_tail_component(tail, || {
        invalid_response!("error_detail.secret.vault_path_required")
    })?;
    reject_option_like_cli_argument("vault", "path", &path)?;
    Ok(SecretSpec::Vault {
        path,
        field: optional_query_value(query, "field").unwrap_or_else(|| "token".to_string()),
        namespace: optional_query_value(query, "namespace"),
    })
}

fn parse_aws_sm_spec(tail: Option<&str>, query: &QueryParameters) -> Result<SecretSpec> {
    ensure_allowed_query_parameters("aws-sm", query, &["region", "profile", "json_key"])?;
    let secret_id = decode_required_tail_component(tail, || {
        invalid_response!("error_detail.secret.aws_secret_id_required")
    })?;
    reject_option_like_cli_argument("aws-sm", "secret_id", &secret_id)?;
    let region = optional_query_value(query, "region");
    if let Some(region) = region.as_deref() {
        reject_option_like_cli_argument("aws-sm", "region", region)?;
    }
    let profile = optional_query_value(query, "profile");
    if let Some(profile) = profile.as_deref() {
        reject_option_like_cli_argument("aws-sm", "profile", profile)?;
    }
    let json_key = optional_query_value(query, "json_key");
    if let Some(key) = json_key.as_deref() {
        validate_json_key_path(key)?;
    }
    Ok(SecretSpec::AwsSecretsManager {
        secret_id,
        region,
        profile,
        json_key,
    })
}

fn parse_gcp_sm_spec(tail: Option<&str>, query: &QueryParameters) -> Result<SecretSpec> {
    ensure_allowed_query_parameters("gcp-sm", query, &["project", "version", "json_key"])?;
    let secret = decode_required_tail_component(tail, || {
        invalid_response!("error_detail.secret.gcp_secret_required")
    })?;
    reject_option_like_cli_argument("gcp-sm", "secret", &secret)?;
    let project = optional_query_value(query, "project");
    if let Some(project) = project.as_deref() {
        reject_option_like_cli_argument("gcp-sm", "project", project)?;
    }
    let version = optional_query_value(query, "version").unwrap_or_else(|| "latest".to_string());
    reject_option_like_cli_argument("gcp-sm", "version", &version)?;
    let json_key = optional_query_value(query, "json_key");
    if let Some(key) = json_key.as_deref() {
        validate_json_key_path(key)?;
    }
    Ok(SecretSpec::GcpSecretManager {
        secret,
        project,
        version,
        json_key,
    })
}

fn parse_azure_kv_spec(tail: Option<&str>, query: &QueryParameters) -> Result<SecretSpec> {
    ensure_allowed_query_parameters("azure-kv", query, &["version"])?;
    let (vault, name) = split_azure_kv_tail(tail)?;
    let version = optional_query_value(query, "version");
    if let Some(version) = version.as_deref() {
        reject_option_like_cli_argument("azure-kv", "version", version)?;
    }
    Ok(SecretSpec::AzureKeyVault {
        vault,
        name,
        version,
    })
}

fn split_azure_kv_tail(tail: Option<&str>) -> Result<(String, String)> {
    let tail = tail.unwrap_or_default();
    let (vault, name) = tail
        .split_once('/')
        .ok_or_else(|| invalid_response!("error_detail.secret.azure_vault_name_required"))?;
    let vault = decode_spec_component(vault.trim())?;
    let name = decode_spec_component(name.trim())?;
    if vault.is_empty() || name.is_empty() {
        return Err(invalid_response!(
            "error_detail.secret.azure_vault_name_required"
        ));
    }
    reject_option_like_cli_argument("azure-kv", "vault", &vault)?;
    reject_option_like_cli_argument("azure-kv", "name", &name)?;
    Ok((vault, name))
}

fn decode_required_tail_component(
    tail: Option<&str>,
    missing_error: impl FnOnce() -> SecretError,
) -> Result<String> {
    let value = decode_spec_component(tail.unwrap_or_default().trim())?;
    if value.is_empty() {
        return Err(missing_error());
    }
    Ok(value)
}

fn optional_query_value(query: &QueryParameters, key: &str) -> Option<String> {
    query
        .get(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn build_secret_command(spec: &SecretSpec) -> Option<SecretCommand> {
    match spec {
        SecretSpec::Env { .. } | SecretSpec::File { .. } => None,
        SecretSpec::Vault {
            path,
            field,
            namespace,
        } => Some(build_vault_command(path, field, namespace.as_deref())),
        SecretSpec::AwsSecretsManager {
            secret_id,
            region,
            profile,
            json_key,
        } => Some(build_aws_sm_command(
            secret_id,
            region.as_deref(),
            profile.as_deref(),
            json_key.clone(),
        )),
        SecretSpec::GcpSecretManager {
            secret,
            project,
            version,
            json_key,
        } => Some(build_gcp_sm_command(
            secret,
            project.as_deref(),
            version,
            json_key.clone(),
        )),
        SecretSpec::AzureKeyVault {
            vault,
            name,
            version,
        } => Some(build_azure_kv_command(vault, name, version.as_deref())),
    }
}

fn build_vault_command(path: &str, field: &str, namespace: Option<&str>) -> SecretCommand {
    let mut env = BTreeMap::new();
    if let Some(namespace) = namespace {
        env.insert("VAULT_NAMESPACE".to_string(), namespace.to_string());
    }

    SecretCommand {
        program: "vault".to_string(),
        args: vec![
            "kv".to_string(),
            "get".to_string(),
            format!("-field={field}"),
            path.to_string(),
        ],
        env,
        json_key: None,
    }
}

fn build_aws_sm_command(
    secret_id: &str,
    region: Option<&str>,
    profile: Option<&str>,
    json_key: Option<String>,
) -> SecretCommand {
    let mut args = vec![
        "secretsmanager".to_string(),
        "get-secret-value".to_string(),
        "--secret-id".to_string(),
        secret_id.to_string(),
        "--query".to_string(),
        "SecretString".to_string(),
        "--output".to_string(),
        "text".to_string(),
    ];
    if let Some(region) = region {
        args.push("--region".to_string());
        args.push(region.to_string());
    }
    if let Some(profile) = profile {
        args.push("--profile".to_string());
        args.push(profile.to_string());
    }

    SecretCommand {
        program: "aws".to_string(),
        args,
        env: BTreeMap::new(),
        json_key,
    }
}

fn build_gcp_sm_command(
    secret: &str,
    project: Option<&str>,
    version: &str,
    json_key: Option<String>,
) -> SecretCommand {
    let mut args = vec![
        "secrets".to_string(),
        "versions".to_string(),
        "access".to_string(),
        version.to_string(),
        "--secret".to_string(),
        secret.to_string(),
    ];
    if let Some(project) = project {
        args.push("--project".to_string());
        args.push(project.to_string());
    }

    SecretCommand {
        program: "gcloud".to_string(),
        args,
        env: BTreeMap::new(),
        json_key,
    }
}

fn build_azure_kv_command(vault: &str, name: &str, version: Option<&str>) -> SecretCommand {
    let mut args = vec![
        "keyvault".to_string(),
        "secret".to_string(),
        "show".to_string(),
        "--vault-name".to_string(),
        vault.to_string(),
        "--name".to_string(),
        name.to_string(),
        "--query".to_string(),
        "value".to_string(),
        "-o".to_string(),
        "tsv".to_string(),
    ];
    if let Some(version) = version {
        args.push("--version".to_string());
        args.push(version.to_string());
    }

    SecretCommand {
        program: "az".to_string(),
        args,
        env: BTreeMap::new(),
        json_key: None,
    }
}

fn parse_query(query: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::<String, String>::new();
    for pair in query.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_query_component(key.trim())?;
        if key.is_empty() {
            continue;
        }
        if out.contains_key(&key) {
            return Err(invalid_response!(
                "error_detail.secret.duplicate_query_parameter",
                "parameter" => key
            ));
        }
        let value = decode_query_component(value.trim())?;
        if value.trim().is_empty() {
            return Err(invalid_response!(
                "error_detail.secret.empty_query_parameter",
                "parameter" => key
            ));
        }
        out.insert(key, value);
    }
    Ok(out)
}

fn ensure_allowed_query_parameters(
    provider: &str,
    query: &BTreeMap<String, String>,
    allowed: &[&str],
) -> Result<()> {
    for key in query.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid_response!(
                "error_detail.secret.unsupported_query_parameter",
                "provider" => provider,
                "parameter" => key
            ));
        }
    }
    Ok(())
}

fn reject_option_like_cli_argument(provider: &str, field: &str, value: &str) -> Result<()> {
    if value.starts_with('-') {
        return Err(invalid_response!(
            "error_detail.secret.option_like_cli_argument",
            "provider" => provider,
            "field" => field
        ));
    }
    Ok(())
}

fn decode_spec_component(raw: &str) -> Result<String> {
    decode_percent_encoded(raw)
}

fn decode_query_component(raw: &str) -> Result<String> {
    decode_percent_encoded(raw)
}

fn decode_percent_encoded(raw: &str) -> Result<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hi = bytes
                    .get(index + 1)
                    .and_then(|byte| decode_hex_nibble(*byte))
                    .ok_or_else(|| {
                        invalid_response!("error_detail.secret.invalid_percent_encoding")
                    })?;
                let lo = bytes
                    .get(index + 2)
                    .and_then(|byte| decode_hex_nibble(*byte))
                    .ok_or_else(|| {
                        invalid_response!("error_detail.secret.invalid_percent_encoding")
                    })?;
                out.push((hi << 4) | lo);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out)
        .map_err(|_| invalid_response!("error_detail.secret.invalid_percent_encoding"))
}

fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
