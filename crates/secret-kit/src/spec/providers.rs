//! Builtin provider wiring stays here so `spec.rs` can keep the generic
//! `secret://` parse/resolve flow without accumulating provider-specific CLI
//! details.

use std::collections::BTreeMap;

use crate::SecretError;
use crate::json::validate_json_key_path;

use super::{
    QueryParameters, Result, SecretCommand, SecretSpec, decode_required_tail_component,
    decode_spec_component, ensure_allowed_query_parameters, optional_query_value,
    reject_option_like_cli_argument,
};

pub(super) fn parse_provider_spec(
    provider: &str,
    tail: Option<&str>,
    query: &QueryParameters,
) -> Result<SecretSpec> {
    match provider {
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

pub(super) fn build_provider_command(spec: &SecretSpec) -> Option<SecretCommand> {
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
