use structured_text_kit::structured_text;

use crate::{LogCode, LogLevel, LogRecord};

#[test]
fn log_code_rejects_invalid_components() {
    assert!(LogCode::try_new("").is_err());
    assert!(LogCode::try_new("notify..hub").is_err());
    assert!(LogCode::try_new("notify hub").is_err());
}

#[test]
fn log_record_rejects_invalid_field_names() {
    let mut record = LogRecord::new(
        LogLevel::Warn,
        LogCode::try_new("notify.hub.dropped").expect("literal code should validate"),
    );

    assert!(record.try_with_field("bad field", "value").is_err());
}

#[test]
fn log_record_display_includes_code_text_target_and_fields() {
    let mut record = LogRecord::new(
        LogLevel::Warn,
        LogCode::try_new("notify.hub.dropped").expect("literal code should validate"),
    )
    .with_target("notify-kit")
    .with_text(structured_text!("notify.hub.dropped", "reason" => "overloaded"));
    record
        .try_with_field("sink", "hub")
        .expect("field name should validate");
    record
        .try_with_field("retryable", false)
        .expect("field name should validate");

    assert_eq!(
        record.to_string(),
        r#"WARN notify.hub.dropped @notify-kit: notify.hub.dropped {reason="overloaded"} {retryable=false, sink="hub"}"#
    );
}

#[test]
fn log_level_maps_to_tracing_level() {
    assert_eq!(LogLevel::Warn.as_tracing_level(), tracing::Level::WARN);
    assert_eq!(LogLevel::Error.as_tracing_level(), tracing::Level::ERROR);
}
