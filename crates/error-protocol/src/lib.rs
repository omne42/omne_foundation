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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ErrorRetryAdviceData {
    DoNotRetry,
    Retryable,
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
            _ => unreachable!("unsupported non-exhaustive ErrorCategory variant"),
        }
    }
}

impl From<ErrorCategoryData> for ErrorCategory {
    fn from(value: ErrorCategoryData) -> Self {
        match value {
            ErrorCategoryData::InvalidInput => Self::InvalidInput,
            ErrorCategoryData::NotFound => Self::NotFound,
            ErrorCategoryData::Conflict => Self::Conflict,
            ErrorCategoryData::PermissionDenied => Self::PermissionDenied,
            ErrorCategoryData::Unauthenticated => Self::Unauthenticated,
            ErrorCategoryData::RateLimited => Self::RateLimited,
            ErrorCategoryData::Timeout => Self::Timeout,
            ErrorCategoryData::Unavailable => Self::Unavailable,
            ErrorCategoryData::ExternalDependency => Self::ExternalDependency,
            ErrorCategoryData::Internal => Self::Internal,
        }
    }
}

impl From<ErrorRetryAdvice> for ErrorRetryAdviceData {
    fn from(value: ErrorRetryAdvice) -> Self {
        match value {
            ErrorRetryAdvice::DoNotRetry => Self::DoNotRetry,
            ErrorRetryAdvice::Retryable => Self::Retryable,
            _ => unreachable!("unsupported non-exhaustive ErrorRetryAdvice variant"),
        }
    }
}

impl From<ErrorRetryAdviceData> for ErrorRetryAdvice {
    fn from(value: ErrorRetryAdviceData) -> Self {
        match value {
            ErrorRetryAdviceData::DoNotRetry => Self::DoNotRetry,
            ErrorRetryAdviceData::Retryable => Self::Retryable,
        }
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
        .with_category(data.category.into())
        .with_retry_advice(data.retry_advice.into());

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
