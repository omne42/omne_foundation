use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use thiserror::Error;

const INVALID_LOG_CODE: &str =
    "log code must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LogCodeValidationError {
    #[error("{INVALID_LOG_CODE}: {0:?}")]
    InvalidCode(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogCode(String);

impl LogCode {
    pub fn try_new(code: impl Into<String>) -> Result<Self, LogCodeValidationError> {
        let code = code.into();
        validate_log_code(&code)?;
        Ok(Self(code))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl AsRef<str> for LogCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for LogCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for LogCode {
    type Error = LogCodeValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl TryFrom<&str> for LogCode {
    type Error = LogCodeValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl FromStr for LogCode {
    type Err = LogCodeValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_new(s)
    }
}

fn validate_log_code(code: &str) -> Result<(), LogCodeValidationError> {
    if code.is_empty() {
        return Err(LogCodeValidationError::InvalidCode(code.to_owned()));
    }

    for component in code.split('.') {
        if component.is_empty()
            || !component
                .bytes()
                .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
        {
            return Err(LogCodeValidationError::InvalidCode(code.to_owned()));
        }
    }

    Ok(())
}
