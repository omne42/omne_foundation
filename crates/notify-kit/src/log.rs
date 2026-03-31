use std::borrow::Cow;
#[cfg(any(feature = "all-sinks", feature = "feishu"))]
use std::path::Path;

#[cfg(any(feature = "all-sinks", feature = "feishu"))]
use http_kit::redact_url_str;
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

fn sanitize_logged_error(error: &str) -> Cow<'_, str> {
    if !error.contains(", response=") {
        return Cow::Borrowed(error);
    }

    let mut out = String::with_capacity(error.len());
    for (idx, line) in error.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(sanitize_logged_error_line(line).as_ref());
    }
    Cow::Owned(out)
}

fn sanitize_logged_error_line(line: &str) -> Cow<'_, str> {
    let Some(response_idx) = line.find(", response=") else {
        return Cow::Borrowed(line);
    };

    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..response_idx]);
    out.push_str(" (response body omitted)");
    Cow::Owned(out)
}

#[cfg(any(feature = "all-sinks", feature = "feishu"))]
fn sanitize_image_src(image_src: &str) -> Cow<'_, str> {
    if image_src.contains("://") {
        return Cow::Owned(redact_url_str(image_src));
    }

    let path = Path::new(image_src);
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return Cow::Borrowed("<local-path>");
    };
    if file_name == image_src {
        return Cow::Borrowed(image_src);
    }

    Cow::Owned(format!("<local-path:{file_name}>"))
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
    push_field(
        &mut record,
        "error",
        sanitize_logged_error(error).into_owned(),
    );
    record.emit_tracing();
}

#[cfg(any(feature = "all-sinks", feature = "feishu"))]
pub(crate) fn warn_feishu_image_load_failed(image_src: &str, error: &str) {
    let mut record = warn_record(
        "notify.feishu.image_load_failed",
        "notify-kit.feishu",
        "feishu image load failed",
    );
    push_field(&mut record, "sink", "feishu");
    push_field(
        &mut record,
        "image_src",
        sanitize_image_src(image_src).into_owned(),
    );
    push_field(
        &mut record,
        "error",
        sanitize_logged_error(error).into_owned(),
    );
    record.emit_tracing();
}

#[cfg(any(feature = "all-sinks", feature = "feishu"))]
pub(crate) fn warn_feishu_image_upload_failed(image_src: &str, error: &str) {
    let mut record = warn_record(
        "notify.feishu.image_upload_failed",
        "notify-kit.feishu",
        "feishu image upload failed",
    );
    push_field(&mut record, "sink", "feishu");
    push_field(
        &mut record,
        "image_src",
        sanitize_image_src(image_src).into_owned(),
    );
    push_field(
        &mut record,
        "error",
        sanitize_logged_error(error).into_owned(),
    );
    record.emit_tracing();
}

#[cfg(all(
    feature = "sound-command",
    any(feature = "all-sinks", feature = "sound")
))]
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

#[cfg(all(
    not(feature = "sound-command"),
    any(feature = "all-sinks", feature = "sound")
))]
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

#[cfg(test)]
mod tests {
    use super::{sanitize_image_src, sanitize_logged_error};

    #[test]
    fn sanitize_logged_error_omits_http_response_preview() {
        let error =
            "generic webhook http error: 500 Internal Server Error, response=secret upstream body";
        assert_eq!(
            sanitize_logged_error(error).as_ref(),
            "generic webhook http error: 500 Internal Server Error (response body omitted)"
        );
    }

    #[test]
    fn sanitize_logged_error_redacts_each_sink_failure_line() {
        let error = "one or more sinks failed:\n- webhook: generic webhook http error: 500 Internal Server Error, response=body\n- slack: timeout after 5s";
        assert_eq!(
            sanitize_logged_error(error).as_ref(),
            "one or more sinks failed:\n- webhook: generic webhook http error: 500 Internal Server Error (response body omitted)\n- slack: timeout after 5s"
        );
    }

    #[cfg(any(feature = "all-sinks", feature = "feishu"))]
    #[test]
    fn sanitize_image_src_redacts_remote_urls() {
        let image_src = "https://example.com/path/to/image.png?token=secret";
        let sanitized = sanitize_image_src(image_src);
        assert_eq!(sanitized.as_ref(), "https://example.com/<redacted>");
        assert!(!sanitized.contains("secret"));
    }

    #[cfg(any(feature = "all-sinks", feature = "feishu"))]
    #[test]
    fn sanitize_image_src_keeps_only_local_file_name() {
        let sanitized = sanitize_image_src("/tmp/private/assets/image.png");
        assert_eq!(sanitized.as_ref(), "<local-path:image.png>");
    }
}
