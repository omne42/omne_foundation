#![forbid(unsafe_code)]

use error_kit::{
    ErrorCategory, ErrorCode, ErrorCodeValidationError, ErrorRecord, ErrorRetryAdvice,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use structured_text_kit::StructuredText;
use structured_text_protocol::{StructuredTextData, StructuredTextDataError};
use thiserror::Error;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ErrorCategoryData {
    InvalidInput,
    NotFound,
    Conflict,
    PermissionDenied,
    Unauthenticated,
    RateLimited,
    Timeout,
    Unavailable,
    ExternalDependency,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ErrorRetryAdviceData {
    DoNotRetry,
    Retryable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct ErrorData {
    pub code: String,
    pub category: ErrorCategoryData,
    pub retry_advice: ErrorRetryAdviceData,
    pub user_text: StructuredTextData,
    pub diagnostic_text: Option<StructuredTextData>,
}

#[derive(Debug, Error)]
pub enum ErrorDataError {
    #[error(transparent)]
    InvalidCode(#[from] ErrorCodeValidationError),
    #[error(transparent)]
    Text(#[from] StructuredTextDataError),
    #[error("error category {value:?} is not representable by error-kit")]
    UnknownCategory { value: ErrorCategoryData },
    #[error("error retry advice {value:?} is not representable by error-kit")]
    UnknownRetryAdvice { value: ErrorRetryAdviceData },
}

impl From<ErrorCategory> for ErrorCategoryData {
    fn from(value: ErrorCategory) -> Self {
        match value {
            ErrorCategory::InvalidInput => Self::InvalidInput,
            ErrorCategory::NotFound => Self::NotFound,
            ErrorCategory::Conflict => Self::Conflict,
            ErrorCategory::PermissionDenied => Self::PermissionDenied,
            ErrorCategory::Unauthenticated => Self::Unauthenticated,
            ErrorCategory::RateLimited => Self::RateLimited,
            ErrorCategory::Timeout => Self::Timeout,
            ErrorCategory::Unavailable => Self::Unavailable,
            ErrorCategory::ExternalDependency => Self::ExternalDependency,
            ErrorCategory::Internal => Self::Internal,
            _ => Self::Unknown,
        }
    }
}

fn try_error_category(value: ErrorCategoryData) -> Result<ErrorCategory, ErrorDataError> {
    match value {
        ErrorCategoryData::InvalidInput => Ok(ErrorCategory::InvalidInput),
        ErrorCategoryData::NotFound => Ok(ErrorCategory::NotFound),
        ErrorCategoryData::Conflict => Ok(ErrorCategory::Conflict),
        ErrorCategoryData::PermissionDenied => Ok(ErrorCategory::PermissionDenied),
        ErrorCategoryData::Unauthenticated => Ok(ErrorCategory::Unauthenticated),
        ErrorCategoryData::RateLimited => Ok(ErrorCategory::RateLimited),
        ErrorCategoryData::Timeout => Ok(ErrorCategory::Timeout),
        ErrorCategoryData::Unavailable => Ok(ErrorCategory::Unavailable),
        ErrorCategoryData::ExternalDependency => Ok(ErrorCategory::ExternalDependency),
        ErrorCategoryData::Internal => Ok(ErrorCategory::Internal),
        ErrorCategoryData::Unknown => Err(ErrorDataError::UnknownCategory { value }),
    }
}

impl From<ErrorRetryAdvice> for ErrorRetryAdviceData {
    fn from(value: ErrorRetryAdvice) -> Self {
        match value {
            ErrorRetryAdvice::DoNotRetry => Self::DoNotRetry,
            ErrorRetryAdvice::Retryable => Self::Retryable,
            _ => Self::Unknown,
        }
    }
}

fn try_error_retry_advice(value: ErrorRetryAdviceData) -> Result<ErrorRetryAdvice, ErrorDataError> {
    match value {
        ErrorRetryAdviceData::DoNotRetry => Ok(ErrorRetryAdvice::DoNotRetry),
        ErrorRetryAdviceData::Retryable => Ok(ErrorRetryAdvice::Retryable),
        ErrorRetryAdviceData::Unknown => Err(ErrorDataError::UnknownRetryAdvice { value }),
    }
}

impl From<&ErrorRecord> for ErrorData {
    fn from(error: &ErrorRecord) -> Self {
        Self {
            code: error.code().to_string(),
            category: error.category().into(),
            retry_advice: error.retry_advice().into(),
            user_text: StructuredTextData::from(error.user_text()),
            diagnostic_text: error.diagnostic_text().map(StructuredTextData::from),
        }
    }
}

impl TryFrom<&ErrorData> for ErrorRecord {
    type Error = ErrorDataError;

    fn try_from(data: &ErrorData) -> Result<Self, Self::Error> {
        let mut error = ErrorRecord::new(
            ErrorCode::try_new(data.code.clone())?,
            StructuredText::try_from(&data.user_text)?,
        )
        .with_category(try_error_category(data.category)?)
        .with_retry_advice(try_error_retry_advice(data.retry_advice)?);

        if let Some(text) = &data.diagnostic_text {
            error = error.with_diagnostic_text(StructuredText::try_from(text)?);
        }

        Ok(error)
    }
}

impl TryFrom<ErrorData> for ErrorRecord {
    type Error = ErrorDataError;

    fn try_from(data: ErrorData) -> Result<Self, Self::Error> {
        Self::try_from(&data)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;
    use structured_text_kit::structured_text;

    #[test]
    fn round_trip_preserves_error_metadata_and_texts() {
        let error = ErrorRecord::new(
            ErrorCode::try_new("secret.lookup_failed").expect("literal code should validate"),
            structured_text!("error_detail.secret.lookup_failed", "provider" => "vault"),
        )
        .with_category(ErrorCategory::Unavailable)
        .with_retry_advice(ErrorRetryAdvice::Retryable)
        .with_diagnostic_text(structured_text!(
            "diagnostic.secret.lookup_failed",
            "attempt" => 3_u8
        ));

        let wire = ErrorData::from(&error);
        assert_eq!(wire.code, "secret.lookup_failed");
        assert_eq!(wire.category, ErrorCategoryData::Unavailable);
        assert_eq!(wire.retry_advice, ErrorRetryAdviceData::Retryable);

        let round_trip = ErrorRecord::try_from(wire).expect("wire payload should deserialize");
        assert_eq!(round_trip.code().as_str(), "secret.lookup_failed");
        assert_eq!(round_trip.category(), ErrorCategory::Unavailable);
        assert_eq!(round_trip.retry_advice(), ErrorRetryAdvice::Retryable);
        assert_eq!(
            round_trip
                .user_text()
                .as_catalog()
                .expect("catalog text")
                .code(),
            "error_detail.secret.lookup_failed"
        );
        assert_eq!(
            round_trip
                .diagnostic_text()
                .expect("diagnostic text")
                .as_catalog()
                .expect("catalog text")
                .code(),
            "diagnostic.secret.lookup_failed"
        );
    }

    #[test]
    fn invalid_code_is_rejected() {
        let err = ErrorRecord::try_from(ErrorData {
            code: "bad code".to_string(),
            category: ErrorCategoryData::Internal,
            retry_advice: ErrorRetryAdviceData::DoNotRetry,
            user_text: StructuredTextData::Freeform {
                text: "plain error".to_string(),
            },
            diagnostic_text: None,
        })
        .expect_err("invalid code should be rejected");

        assert!(matches!(err, ErrorDataError::InvalidCode(_)));
    }

    #[test]
    fn unknown_protocol_category_is_rejected() {
        let err = ErrorRecord::try_from(ErrorData {
            code: "secret.lookup_failed".to_string(),
            category: ErrorCategoryData::Unknown,
            retry_advice: ErrorRetryAdviceData::Retryable,
            user_text: StructuredTextData::Freeform {
                text: "plain error".to_string(),
            },
            diagnostic_text: None,
        })
        .expect_err("unknown category should not silently rewrite semantics");

        assert!(matches!(
            err,
            ErrorDataError::UnknownCategory {
                value: ErrorCategoryData::Unknown
            }
        ));
    }

    #[test]
    fn unknown_protocol_retry_advice_is_rejected() {
        let err = ErrorRecord::try_from(ErrorData {
            code: "secret.lookup_failed".to_string(),
            category: ErrorCategoryData::Unavailable,
            retry_advice: ErrorRetryAdviceData::Unknown,
            user_text: StructuredTextData::Freeform {
                text: "plain error".to_string(),
            },
            diagnostic_text: None,
        })
        .expect_err("unknown retry advice should not silently rewrite semantics");

        assert!(matches!(
            err,
            ErrorDataError::UnknownRetryAdvice {
                value: ErrorRetryAdviceData::Unknown
            }
        ));
    }

    #[test]
    fn protocol_drops_runtime_source_chain() {
        let error = ErrorRecord::new(
            ErrorCode::try_new("secret.lookup_failed").expect("literal code should validate"),
            structured_text!("error_detail.secret.lookup_failed"),
        )
        .with_source(std::io::Error::other("boom"));

        let wire = ErrorData::from(&error);
        let round_trip = ErrorRecord::try_from(wire).expect("wire payload should deserialize");

        assert!(round_trip.source().is_none());
    }
}
