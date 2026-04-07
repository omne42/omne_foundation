use log_kit::{LogCode, LogLevel, LogRecord, LogValue};
use structured_text_kit::StructuredText;

fn warn_record(code: &'static str, target: &'static str, text: &'static str) -> LogRecord {
    LogRecord::new(
        LogLevel::Warn,
        LogCode::try_new(code).expect("literal log code should validate"),
    )
    .with_target(target)
    .with_text(StructuredText::freeform(text))
}

fn push_field(record: &mut LogRecord, name: &'static str, value: impl Into<LogValue>) {
    record
        .try_with_field(name, value)
        .expect("literal log field name should validate");
}

pub(crate) fn warn_hub_notify_dropped(kind: &str, reason: &'static str) {
    let text = match reason {
        "no_tokio_runtime" => "notify dropped: no tokio runtime",
        "overloaded" => "notify dropped: overloaded",
        _ => "notify dropped",
    };
    let mut record = warn_record("notify.hub.notify_dropped", "notify-kit.hub", text);
    push_field(&mut record, "sink", "hub");
    push_field(&mut record, "kind", kind);
    push_field(&mut record, "reason", reason);
    record.emit_tracing();
}

pub(crate) fn warn_hub_notify_failed(kind: &str, error: &str) {
    let mut record = warn_record(
        "notify.hub.notify_failed",
        "notify-kit.hub",
        "notify failed",
    );
    push_field(&mut record, "sink", "hub");
    push_field(&mut record, "kind", kind);
    push_field(&mut record, "error", error);
    record.emit_tracing();
}

#[cfg(feature = "sink-feishu")]
pub(crate) fn warn_feishu_image_load_failed(image_src: &str, error: &str) {
    let mut record = warn_record(
        "notify.feishu.image_load_failed",
        "notify-kit.feishu",
        "feishu image load failed",
    );
    push_field(&mut record, "sink", "feishu");
    push_field(&mut record, "image_src", image_src);
    push_field(&mut record, "error", error);
    record.emit_tracing();
}

#[cfg(feature = "sink-feishu")]
pub(crate) fn warn_feishu_image_upload_failed(image_src: &str, error: &str) {
    let mut record = warn_record(
        "notify.feishu.image_upload_failed",
        "notify-kit.feishu",
        "feishu image upload failed",
    );
    push_field(&mut record, "sink", "feishu");
    push_field(&mut record, "image_src", image_src);
    push_field(&mut record, "error", error);
    record.emit_tracing();
}

#[cfg(feature = "sound-command")]
pub(crate) fn warn_sound_command_exited_non_zero(program: &str, status: &str) {
    let mut record = warn_record(
        "notify.sound.command_exited_non_zero",
        "notify-kit.sound",
        "sound command exited non-zero",
    );
    push_field(&mut record, "sink", "sound");
    push_field(&mut record, "program", program);
    push_field(&mut record, "status", status);
    record.emit_tracing();
}

#[cfg(all(feature = "sink-sound", not(feature = "sound-command")))]
pub(crate) fn warn_sound_command_disabled_fallback() {
    let mut record = warn_record(
        "notify.sound.command_disabled_fallback",
        "notify-kit.sound",
        "sound command_argv configured but feature \"sound-command\" is disabled; falling back to terminal bell",
    );
    push_field(&mut record, "sink", "sound");
    push_field(&mut record, "reason", "feature_disabled");
    record.emit_tracing();
}
