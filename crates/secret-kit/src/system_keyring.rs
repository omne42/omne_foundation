use structured_text_kit::structured_text;

use crate::{Result, SecretError, SecretString};

pub(crate) async fn read_keyring_secret(service: &str, account: &str) -> Result<SecretString> {
    let service = service.to_string();
    let account = account.to_string();
    tokio::task::spawn_blocking(move || read_keyring_secret_blocking(&service, &account))
        .await
        .map_err(|err| {
            secret_provider_error!(
                "error_detail.secret.keyring_blocking_task_failed",
                "error" => err.to_string()
            )
        })?
}

fn read_keyring_secret_blocking(service: &str, account: &str) -> Result<SecretString> {
    let entry = keyring::Entry::new(service, account)
        .map_err(|err| map_keyring_error("create", service, account, err))?;
    let password = entry
        .get_password()
        .map_err(|err| map_keyring_error("get", service, account, err))?;
    Ok(SecretString::from(password))
}

fn map_keyring_error(
    operation: &str,
    service: &str,
    account: &str,
    error: keyring::Error,
) -> SecretError {
    match error {
        keyring::Error::NoEntry => SecretError::lookup(structured_text!(
            "error_detail.secret.keyring_no_entry",
            "service" => service,
            "account" => account
        )),
        keyring::Error::NoStorageAccess(err) => secret_provider_error!(
            "error_detail.secret.keyring_no_storage_access",
            "operation" => operation,
            "service" => service,
            "account" => account,
            "error" => err.to_string()
        ),
        keyring::Error::PlatformFailure(err) => secret_provider_error!(
            "error_detail.secret.keyring_platform_failure",
            "operation" => operation,
            "service" => service,
            "account" => account,
            "error" => err.to_string()
        ),
        keyring::Error::BadEncoding(_) => secret_provider_error!(
            "error_detail.secret.keyring_bad_encoding",
            "operation" => operation,
            "service" => service,
            "account" => account
        ),
        keyring::Error::TooLong(name, len) => secret_provider_error!(
            "error_detail.secret.keyring_entry_too_long",
            "operation" => operation,
            "service" => service,
            "account" => account,
            "field" => name,
            "limit" => len.to_string()
        ),
        keyring::Error::Invalid(attribute, reason) => secret_provider_error!(
            "error_detail.secret.keyring_invalid_entry",
            "operation" => operation,
            "service" => service,
            "account" => account,
            "field" => attribute,
            "reason" => reason
        ),
        keyring::Error::Ambiguous(items) => secret_provider_error!(
            "error_detail.secret.keyring_ambiguous_entry",
            "operation" => operation,
            "service" => service,
            "account" => account,
            "count" => items.len().to_string()
        ),
        other => secret_provider_error!(
            "error_detail.secret.keyring_provider_error",
            "operation" => operation,
            "service" => service,
            "account" => account,
            "error" => other.to_string()
        ),
    }
}
