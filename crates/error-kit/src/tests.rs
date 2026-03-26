use std::error::Error as _;

use structured_text_kit::structured_text;

#[cfg(feature = "cli")]
use crate::CliExitCode;
use crate::{ErrorCategory, ErrorCode, ErrorRecord, ErrorRetryAdvice};

#[test]
fn error_code_rejects_invalid_components() {
    assert!(ErrorCode::try_new("").is_err());
    assert!(ErrorCode::try_new("secret..lookup").is_err());
    assert!(ErrorCode::try_new("secret lookup").is_err());
}

#[test]
fn error_record_defaults_to_internal_non_retryable() {
    let error = ErrorRecord::new(
        ErrorCode::try_new("secret.lookup_failed").expect("literal code should validate"),
        structured_text!("error_detail.secret.lookup_failed"),
    );

    assert_eq!(error.category(), ErrorCategory::Internal);
    assert_eq!(error.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    assert_eq!(error.code().as_str(), "secret.lookup_failed");
}

#[test]
fn error_record_display_prefers_diagnostic_text_when_present() {
    let error = ErrorRecord::new(
        ErrorCode::try_new("secret.lookup_failed").expect("literal code should validate"),
        structured_text!("error_detail.secret.lookup_failed"),
    )
    .with_diagnostic_text(structured_text!(
        "diagnostic.secret.lookup_failed",
        "provider" => "vault"
    ));

    assert_eq!(
        error.to_string(),
        r#"secret.lookup_failed: diagnostic.secret.lookup_failed {provider="vault"}"#
    );
}

#[test]
fn error_record_exposes_causal_source() {
    let error = ErrorRecord::new(
        ErrorCode::try_new("secret.lookup_failed").expect("literal code should validate"),
        structured_text!("error_detail.secret.lookup_failed"),
    )
    .with_source(std::io::Error::other("boom"));

    assert_eq!(
        error
            .source()
            .expect("source should be present")
            .to_string(),
        "boom"
    );
}

#[test]
fn error_record_supports_freeform_construction() {
    let error = ErrorRecord::new_freeform(
        ErrorCode::try_new("tool.install_failed").expect("literal code should validate"),
        "install failed",
    )
    .with_freeform_diagnostic_text("write target failed");

    assert_eq!(error.user_text().freeform_text(), Some("install failed"));
    assert_eq!(
        error
            .diagnostic_text()
            .and_then(|text| text.freeform_text()),
        Some("write target failed")
    );
}

#[cfg(feature = "cli")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestExitCode {
    Failure = 9,
}

#[cfg(feature = "cli")]
impl CliExitCode for TestExitCode {
    fn as_i32(self) -> i32 {
        self as i32
    }
}

#[cfg(feature = "cli")]
#[test]
fn cli_error_wraps_record_and_exposes_exit_code() {
    let error = ErrorRecord::new_freeform(
        ErrorCode::try_new("tool.download_failed").expect("literal code should validate"),
        "download failed",
    )
    .with_exit_code(TestExitCode::Failure);

    assert_eq!(error.exit_code(), TestExitCode::Failure);
    assert_eq!(error.record().code().as_str(), "tool.download_failed");
}

#[cfg(feature = "cli")]
#[test]
fn cli_error_display_uses_record_display_text() {
    let error = ErrorRecord::new_freeform(
        ErrorCode::try_new("tool.install_failed").expect("literal code should validate"),
        "install failed",
    )
    .with_exit_code(TestExitCode::Failure);

    assert_eq!(error.to_string(), "install failed");
}

#[cfg(feature = "cli")]
#[test]
fn cli_error_sources_the_wrapped_record() {
    let error = ErrorRecord::new_freeform(
        ErrorCode::try_new("tool.install_failed").expect("literal code should validate"),
        "install failed",
    )
    .with_source(std::io::Error::other("boom"))
    .with_exit_code(TestExitCode::Failure);

    assert_eq!(
        error
            .source()
            .expect("cli error should expose wrapped record")
            .to_string(),
        r#"tool.install_failed: @freeform("install failed")"#
    );
}
