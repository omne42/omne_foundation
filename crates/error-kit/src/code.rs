use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use thiserror::Error;

const INVALID_ERROR_CODE: &str =
    "error code must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ErrorCodeValidationError {
    #[error("{INVALID_ERROR_CODE}: {0:?}")]
    InvalidCode(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCode(String);

impl ErrorCode {
    pub fn try_new(code: impl Into<String>) -> Result<Self, ErrorCodeValidationError> {
        let code = code.into();
        validate_error_code(&code)?;
        Ok(Self(code))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl AsRef<str> for ErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ErrorCode> for String {
    fn from(code: ErrorCode) -> Self {
        code.0
    }
}

impl TryFrom<String> for ErrorCode {
    type Error = ErrorCodeValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl TryFrom<&str> for ErrorCode {
    type Error = ErrorCodeValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl FromStr for ErrorCode {
    type Err = ErrorCodeValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_new(s)
    }
}

fn validate_error_code(code: &str) -> Result<(), ErrorCodeValidationError> {
    if code.is_empty() {
        return Err(ErrorCodeValidationError::InvalidCode(code.to_owned()));
    }

    for component in code.split('.') {
        if component.is_empty()
            || !component
                .bytes()
                .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
        {
            return Err(ErrorCodeValidationError::InvalidCode(code.to_owned()));
        }
    }

    Ok(())
}
