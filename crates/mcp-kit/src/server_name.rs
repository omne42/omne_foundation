use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use error_kit::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use structured_text_kit::StructuredText;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerName(Arc<str>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ServerNameError {
    #[error("server name must not be empty")]
    Empty,
    #[error("invalid server name: {0} (allowed: [A-Za-z0-9_-]+)")]
    Invalid(String),
}

impl ServerNameError {
    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::Empty => literal_error_code("mcp_kit.server_name.empty"),
            Self::Invalid(_) => literal_error_code("mcp_kit.server_name.invalid"),
        }
    }

    #[must_use]
    pub fn error_category(&self) -> ErrorCategory {
        ErrorCategory::InvalidInput
    }

    #[must_use]
    pub fn retry_advice(&self) -> ErrorRetryAdvice {
        ErrorRetryAdvice::DoNotRetry
    }

    #[must_use]
    pub fn error_record(&self) -> ErrorRecord {
        ErrorRecord::new(
            self.error_code(),
            StructuredText::freeform(self.to_string()),
        )
        .with_category(self.error_category())
        .with_retry_advice(self.retry_advice())
    }

    #[must_use]
    pub fn into_error_record(self) -> ErrorRecord {
        ErrorRecord::new(
            self.error_code(),
            StructuredText::freeform(self.to_string()),
        )
        .with_category(self.error_category())
        .with_retry_advice(self.retry_advice())
    }
}

impl ServerName {
    /// Parse and validate an MCP server name.
    ///
    /// Note: this trims leading/trailing whitespace before validation. In other words, `" a "`
    /// and `"a"` normalize to the same `ServerName`.
    pub fn parse(name: impl AsRef<str>) -> Result<Self, ServerNameError> {
        let name = name.as_ref().trim();
        if name.is_empty() {
            return Err(ServerNameError::Empty);
        }
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            return Err(ServerNameError::Invalid(name.to_string()));
        }
        Ok(Self(Arc::from(name)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for ServerName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for ServerName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ServerName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ServerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl Serialize for ServerName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ServerName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::parse(name).map_err(D::Error::custom)
    }
}

impl TryFrom<&str> for ServerName {
    type Error = ServerNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for ServerName {
    type Error = ServerNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ServerName> for String {
    fn from(value: ServerName) -> Self {
        value.0.as_ref().to_string()
    }
}

impl From<ServerNameError> for ErrorRecord {
    fn from(error: ServerNameError) -> Self {
        error.into_error_record()
    }
}

fn literal_error_code(code: &'static str) -> ErrorCode {
    ErrorCode::try_new(code).expect("literal error code should validate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_server_name_maps_to_structured_error_record() {
        let error = ServerNameError::Empty;

        let record = error.error_record();

        assert_eq!(record.code().as_str(), "mcp_kit.server_name.empty");
        assert_eq!(record.category(), ErrorCategory::InvalidInput);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("server name must not be empty")
        );
    }

    #[test]
    fn invalid_server_name_maps_to_structured_error_record() {
        let error = ServerNameError::Invalid(String::from("bad name"));

        let record = error.into_error_record();

        assert_eq!(record.code().as_str(), "mcp_kit.server_name.invalid");
        assert_eq!(record.category(), ErrorCategory::InvalidInput);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("invalid server name: bad name (allowed: [A-Za-z0-9_-]+)")
        );
    }
}
