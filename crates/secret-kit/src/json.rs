use structured_text_kit::structured_text;
use zeroize::Zeroize;

use crate::{Result, SecretError, SecretString};

#[cfg(test)]
pub(crate) fn extract_json_key(json: &str, key: &str) -> Result<SecretString> {
    extract_json_key_secret(SecretString::from(json), key)
}

pub(crate) fn extract_json_key_secret(json: SecretString, key: &str) -> Result<SecretString> {
    validate_json_key_path(key)?;
    let value: serde_json::Value = serde_json::from_str(json.expose_secret()).map_err(|err| {
        secret_json_error!(
            "error_detail.secret.json_parse_failed",
            err,
            "key" => key
        )
    })?;
    drop(json);
    extract_json_key_value(value, key)
}

pub(crate) fn validate_json_key_path(key: &str) -> Result<()> {
    if key.split('.').any(|part| part.trim().is_empty()) {
        return Err(invalid_response!(
            "error_detail.secret.invalid_json_key_path",
            "key" => key
        ));
    }
    Ok(())
}

fn extract_json_key_value(mut value: serde_json::Value, key: &str) -> Result<SecretString> {
    let mut resolved_path = String::new();
    for part in key.split('.').map(str::trim).filter(|p| !p.is_empty()) {
        if !resolved_path.is_empty() {
            resolved_path.push('.');
        }
        resolved_path.push_str(part);
        let serde_json::Value::Object(mut fields) = value else {
            zeroize_json_value(value);
            return Err(missing_json_key(resolved_path.as_str()));
        };
        let Some(next) = fields.remove(part) else {
            zeroize_json_value(serde_json::Value::Object(fields));
            return Err(missing_json_key(resolved_path.as_str()));
        };
        zeroize_json_value(serde_json::Value::Object(fields));
        value = next;
    }
    match value {
        serde_json::Value::Null => Err(missing_json_key(if resolved_path.is_empty() {
            key
        } else {
            resolved_path.as_str()
        })),
        serde_json::Value::String(value) => Ok(SecretString::from(value)),
        other => {
            let serialized = match serde_json::to_string(&other) {
                Ok(serialized) => serialized,
                Err(err) => {
                    zeroize_json_value(other);
                    return Err(secret_json_error!(
                        "error_detail.secret.json_serialize_failed",
                        err
                    ));
                }
            };
            zeroize_json_value(other);
            Ok(SecretString::from(serialized))
        }
    }
}

fn zeroize_json_value(value: serde_json::Value) {
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            }
            serde_json::Value::String(mut value) => value.zeroize(),
            serde_json::Value::Array(values) => stack.extend(values),
            serde_json::Value::Object(values) => {
                for (mut key, value) in values {
                    key.zeroize();
                    stack.push(value);
                }
            }
        }
    }
}

fn missing_json_key(key: &str) -> SecretError {
    SecretError::lookup(structured_text!(
        "error_detail.secret.json_missing_key",
        "key" => key
    ))
}
