use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use structured_text_kit::structured_text;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Metadata, Subscriber};

use crate::record::{
    OVERFLOW_FIELDS_FIELD, OVERFLOW_TARGET_FIELD, OVERFLOW_TRACING_TARGET,
    dynamic_callsite_cache_len, max_dynamic_callsites,
};
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

#[test]
fn emit_tracing_uses_real_target_and_flat_fields() {
    let mut record = LogRecord::new(
        LogLevel::Warn,
        LogCode::try_new("notify.hub.dropped").expect("literal code should validate"),
    )
    .with_target("notify-kit.hub")
    .with_text(structured_text!("notify.hub.dropped", "reason" => "overloaded"));
    record
        .try_with_field("sink", "hub")
        .expect("field name should validate");
    record
        .try_with_field("retryable", false)
        .expect("field name should validate");

    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        events: Arc::clone(&events),
    };
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || record.emit_tracing());

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(events.len(), 1, "expected exactly one tracing event");
    let event = &events[0];
    assert_eq!(event.target, "notify-kit.hub");
    assert_eq!(
        event.fields.get("log_code"),
        Some(&CapturedValue::Str("notify.hub.dropped".to_string()))
    );
    assert_eq!(
        event.fields.get("text"),
        Some(&CapturedValue::Str(
            r#"notify.hub.dropped {reason="overloaded"}"#.to_string()
        ))
    );
    assert_eq!(
        event.fields.get("sink"),
        Some(&CapturedValue::Str("hub".to_string()))
    );
    assert_eq!(
        event.fields.get("retryable"),
        Some(&CapturedValue::Bool(false))
    );
    assert!(
        !event.fields.contains_key("fields"),
        "flattened tracing event must not expose a synthetic fields blob"
    );
    assert!(
        !event.fields.contains_key("log_target"),
        "target should live in tracing metadata, not a synthetic field"
    );
}

#[test]
fn dynamic_callsite_cache_stops_growing_at_capacity() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        events: Arc::clone(&events),
    };
    let dispatch = tracing::Dispatch::new(subscriber);

    let initial = dynamic_callsite_cache_len();
    let remaining = max_dynamic_callsites().saturating_sub(initial);

    tracing::dispatcher::with_default(&dispatch, || {
        for index in 0..=remaining {
            let mut record = LogRecord::new(
                LogLevel::Info,
                LogCode::try_new("notify.hub.dropped").expect("literal code should validate"),
            )
            .with_target(format!("notify-kit.cache-{index}"))
            .with_text(structured_text!("notify.hub.dropped", "reason" => "overloaded"));
            record
                .try_with_field(format!("field_{index}"), index as u64)
                .expect("field name should validate");
            record.emit_tracing();
        }
    });

    let final_len = dynamic_callsite_cache_len();
    assert_eq!(final_len, max_dynamic_callsites());

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let overflow_event = events.last().expect("expected captured overflow event");
    assert_eq!(overflow_event.target, OVERFLOW_TRACING_TARGET);
    assert_eq!(
        overflow_event.fields.get("log_code"),
        Some(&CapturedValue::Str("notify.hub.dropped".to_string()))
    );
    assert_eq!(
        overflow_event.fields.get("text"),
        Some(&CapturedValue::Str(
            r#"notify.hub.dropped {reason="overloaded"}"#.to_string()
        ))
    );
    assert_eq!(
        overflow_event.fields.get(OVERFLOW_TARGET_FIELD),
        Some(&CapturedValue::Str(format!("notify-kit.cache-{remaining}")))
    );
    assert_eq!(
        overflow_event.fields.get(OVERFLOW_FIELDS_FIELD),
        Some(&CapturedValue::Str(format!(
            r#"field_{remaining}={remaining}"#
        )))
    );
    assert!(
        !overflow_event
            .fields
            .contains_key(&format!("field_{remaining}")),
        "overflow fallback should stop introducing new dynamic field metadata"
    );
}

#[derive(Debug, Default)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, CapturedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CapturedValue {
    Str(String),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    Debug(String),
}

#[derive(Clone)]
struct CapturingSubscriber {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut captured = CapturedEvent {
            target: event.metadata().target().to_string(),
            fields: BTreeMap::new(),
        };
        let mut visitor = CapturingVisitor {
            fields: &mut captured.fields,
        };
        event.record(&mut visitor);
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(captured);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

struct CapturingVisitor<'a> {
    fields: &'a mut BTreeMap<String, CapturedValue>,
}

impl Visit for CapturingVisitor<'_> {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            CapturedValue::Signed(i128::from(value)),
        );
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            CapturedValue::Unsigned(u128::from(value)),
        );
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.fields
            .insert(field.name().to_string(), CapturedValue::Signed(value));
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.fields
            .insert(field.name().to_string(), CapturedValue::Unsigned(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), CapturedValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(
            field.name().to_string(),
            CapturedValue::Str(value.to_string()),
        );
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields.insert(
            field.name().to_string(),
            CapturedValue::Debug(format!("{value:?}")),
        );
    }
}
