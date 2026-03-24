use crate::text::StructuredText;

pub(crate) const MAX_TEXT_DEPTH: usize = 32;
pub(crate) const INVALID_TEXT_CODE: &str =
    "structured text code must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'";
pub(crate) const INVALID_TEXT_ARG_NAME: &str =
    "structured text arg name must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'";
pub(crate) const DUPLICATE_TEXT_ARG_NAME: &str = "structured text arg names must be unique";

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StructuredTextValidationError {
    InvalidCode(String),
    InvalidArgName(String),
    DuplicateArgName(String),
    NestingTooDeep { max_depth: usize },
}

pub(crate) fn validate_text_code(
    code: impl Into<String>,
) -> Result<String, StructuredTextValidationError> {
    validate_text_component(code, StructuredTextValidationError::InvalidCode)
}

pub(crate) fn validate_text_arg_name(
    name: impl Into<String>,
) -> Result<String, StructuredTextValidationError> {
    validate_text_component(name, StructuredTextValidationError::InvalidArgName)
}

pub(crate) fn validate_nested_text(
    text: &StructuredText,
) -> Result<(), StructuredTextValidationError> {
    if text.exceeds_max_depth(2) {
        return Err(StructuredTextValidationError::NestingTooDeep {
            max_depth: MAX_TEXT_DEPTH,
        });
    }

    Ok(())
}

fn validate_text_component(
    value: impl Into<String>,
    invalid: fn(String) -> StructuredTextValidationError,
) -> Result<String, StructuredTextValidationError> {
    let value = value.into();
    if is_valid_text_component(value.as_str()) {
        return Ok(value);
    }

    Err(invalid(value))
}

const fn is_valid_text_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    let mut index = 0;
    let mut in_segment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        let is_ascii_alphanumeric =
            byte.is_ascii_digit() || byte.is_ascii_lowercase() || byte.is_ascii_uppercase();
        if is_ascii_alphanumeric || matches!(byte, b'_' | b'-') {
            in_segment = true;
        } else if byte == b'.' {
            if !in_segment {
                return false;
            }
            in_segment = false;
        } else {
            return false;
        }
        index += 1;
    }

    in_segment
}

#[doc(hidden)]
#[must_use]
pub const fn __structured_text_component_is_valid(value: &str) -> bool {
    is_valid_text_component(value)
}

const fn literal_strings_equal(lhs: &str, rhs: &str) -> bool {
    let lhs = lhs.as_bytes();
    let rhs = rhs.as_bytes();
    if lhs.len() != rhs.len() {
        return false;
    }

    let mut index = 0;
    while index < lhs.len() {
        if lhs[index] != rhs[index] {
            return false;
        }
        index += 1;
    }

    true
}

#[doc(hidden)]
#[must_use]
pub const fn __structured_text_literals_equal(lhs: &str, rhs: &str) -> bool {
    literal_strings_equal(lhs, rhs)
}
