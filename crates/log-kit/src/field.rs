use std::fmt::{self, Display, Formatter};

use structured_text_kit::StructuredText;
use thiserror::Error;

const INVALID_FIELD_NAME: &str =
    "log field name must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LogFieldNameValidationError {
    #[error("{INVALID_FIELD_NAME}: {0:?}")]
    InvalidName(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogValue {
    Text(String),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    StructuredText(StructuredText),
}

impl LogValue {
    pub(crate) fn validate_field_name(name: &str) -> Result<(), LogFieldNameValidationError> {
        if name.is_empty() {
            return Err(LogFieldNameValidationError::InvalidName(name.to_owned()));
        }

        for component in name.split('.') {
            if component.is_empty()
                || !component.bytes().all(
                    |byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'),
                )
            {
                return Err(LogFieldNameValidationError::InvalidName(name.to_owned()));
            }
        }

        Ok(())
    }
}

impl Display for LogValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => write!(f, "{value:?}"),
            Self::Bool(value) => Display::fmt(value, f),
            Self::Signed(value) => Display::fmt(value, f),
            Self::Unsigned(value) => Display::fmt(value, f),
            Self::StructuredText(value) => write!(f, "{}", value.diagnostic_display()),
        }
    }
}

impl From<String> for LogValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for LogValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<bool> for LogValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i8> for LogValue {
    fn from(value: i8) -> Self {
        Self::Signed(i128::from(value))
    }
}

impl From<i16> for LogValue {
    fn from(value: i16) -> Self {
        Self::Signed(i128::from(value))
    }
}

impl From<i32> for LogValue {
    fn from(value: i32) -> Self {
        Self::Signed(i128::from(value))
    }
}

impl From<i64> for LogValue {
    fn from(value: i64) -> Self {
        Self::Signed(i128::from(value))
    }
}

impl From<i128> for LogValue {
    fn from(value: i128) -> Self {
        Self::Signed(value)
    }
}

impl From<u8> for LogValue {
    fn from(value: u8) -> Self {
        Self::Unsigned(u128::from(value))
    }
}

impl From<u16> for LogValue {
    fn from(value: u16) -> Self {
        Self::Unsigned(u128::from(value))
    }
}

impl From<u32> for LogValue {
    fn from(value: u32) -> Self {
        Self::Unsigned(u128::from(value))
    }
}

impl From<u64> for LogValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(u128::from(value))
    }
}

impl From<u128> for LogValue {
    fn from(value: u128) -> Self {
        Self::Unsigned(value)
    }
}

impl From<StructuredText> for LogValue {
    fn from(value: StructuredText) -> Self {
        Self::StructuredText(value)
    }
}
