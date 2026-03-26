use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};

use structured_text_kit::StructuredText;

use crate::{LogCode, LogFieldNameValidationError, LogValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    level: LogLevel,
    code: LogCode,
    target: Option<String>,
    text: Option<StructuredText>,
    fields: BTreeMap<String, LogValue>,
}

impl LogLevel {
    #[must_use]
    pub fn as_tracing_level(self) -> tracing::Level {
        match self {
            Self::Trace => tracing::Level::TRACE,
            Self::Debug => tracing::Level::DEBUG,
            Self::Info => tracing::Level::INFO,
            Self::Warn => tracing::Level::WARN,
            Self::Error => tracing::Level::ERROR,
        }
    }

    #[must_use]
    fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

impl LogRecord {
    #[must_use]
    pub fn new(level: LogLevel, code: LogCode) -> Self {
        Self {
            level,
            code,
            target: None,
            text: None,
            fields: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    #[must_use]
    pub fn with_text(mut self, text: StructuredText) -> Self {
        self.text = Some(text);
        self
    }

    pub fn try_with_field(
        &mut self,
        name: impl Into<String>,
        value: impl Into<LogValue>,
    ) -> Result<(), LogFieldNameValidationError> {
        let name = name.into();
        LogValue::validate_field_name(&name)?;
        self.fields.insert(name, value.into());
        Ok(())
    }

    #[must_use]
    pub fn level(&self) -> LogLevel {
        self.level
    }

    #[must_use]
    pub fn code(&self) -> &LogCode {
        &self.code
    }

    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    #[must_use]
    pub fn text(&self) -> Option<&StructuredText> {
        self.text.as_ref()
    }

    #[must_use]
    pub fn fields(&self) -> &BTreeMap<String, LogValue> {
        &self.fields
    }

    pub fn emit_tracing(&self) {
        match self.text.as_ref() {
            Some(text) => {
                let rendered = text.diagnostic_display().to_string();
                self.emit_tracing_with_text(&rendered);
            }
            None => self.emit_tracing_without_text(),
        }
    }

    fn emit_tracing_with_text(&self, rendered_text: &str) {
        match self.level {
            LogLevel::Trace => emit_trace(self, Some(rendered_text)),
            LogLevel::Debug => emit_debug(self, Some(rendered_text)),
            LogLevel::Info => emit_info(self, Some(rendered_text)),
            LogLevel::Warn => emit_warn(self, Some(rendered_text)),
            LogLevel::Error => emit_error(self, Some(rendered_text)),
        }
    }

    fn emit_tracing_without_text(&self) {
        match self.level {
            LogLevel::Trace => emit_trace(self, None),
            LogLevel::Debug => emit_debug(self, None),
            LogLevel::Info => emit_info(self, None),
            LogLevel::Warn => emit_warn(self, None),
            LogLevel::Error => emit_error(self, None),
        }
    }
}

fn emit_trace(record: &LogRecord, rendered_text: Option<&str>) {
    match (record.target.as_deref(), rendered_text) {
        (Some(target), Some(text)) => tracing::event!(
            tracing::Level::TRACE,
            log_code = %record.code,
            log_target = %target,
            text = %text,
            fields = ?record.fields
        ),
        (Some(target), None) => tracing::event!(
            tracing::Level::TRACE,
            log_code = %record.code,
            log_target = %target,
            fields = ?record.fields
        ),
        (None, Some(text)) => tracing::event!(
            tracing::Level::TRACE,
            log_code = %record.code,
            text = %text,
            fields = ?record.fields
        ),
        (None, None) => tracing::event!(
            tracing::Level::TRACE,
            log_code = %record.code,
            fields = ?record.fields
        ),
    }
}

fn emit_debug(record: &LogRecord, rendered_text: Option<&str>) {
    match (record.target.as_deref(), rendered_text) {
        (Some(target), Some(text)) => tracing::event!(
            tracing::Level::DEBUG,
            log_code = %record.code,
            log_target = %target,
            text = %text,
            fields = ?record.fields
        ),
        (Some(target), None) => tracing::event!(
            tracing::Level::DEBUG,
            log_code = %record.code,
            log_target = %target,
            fields = ?record.fields
        ),
        (None, Some(text)) => tracing::event!(
            tracing::Level::DEBUG,
            log_code = %record.code,
            text = %text,
            fields = ?record.fields
        ),
        (None, None) => tracing::event!(
            tracing::Level::DEBUG,
            log_code = %record.code,
            fields = ?record.fields
        ),
    }
}

fn emit_info(record: &LogRecord, rendered_text: Option<&str>) {
    match (record.target.as_deref(), rendered_text) {
        (Some(target), Some(text)) => tracing::event!(
            tracing::Level::INFO,
            log_code = %record.code,
            log_target = %target,
            text = %text,
            fields = ?record.fields
        ),
        (Some(target), None) => tracing::event!(
            tracing::Level::INFO,
            log_code = %record.code,
            log_target = %target,
            fields = ?record.fields
        ),
        (None, Some(text)) => tracing::event!(
            tracing::Level::INFO,
            log_code = %record.code,
            text = %text,
            fields = ?record.fields
        ),
        (None, None) => tracing::event!(
            tracing::Level::INFO,
            log_code = %record.code,
            fields = ?record.fields
        ),
    }
}

fn emit_warn(record: &LogRecord, rendered_text: Option<&str>) {
    match (record.target.as_deref(), rendered_text) {
        (Some(target), Some(text)) => tracing::event!(
            tracing::Level::WARN,
            log_code = %record.code,
            log_target = %target,
            text = %text,
            fields = ?record.fields
        ),
        (Some(target), None) => tracing::event!(
            tracing::Level::WARN,
            log_code = %record.code,
            log_target = %target,
            fields = ?record.fields
        ),
        (None, Some(text)) => tracing::event!(
            tracing::Level::WARN,
            log_code = %record.code,
            text = %text,
            fields = ?record.fields
        ),
        (None, None) => tracing::event!(
            tracing::Level::WARN,
            log_code = %record.code,
            fields = ?record.fields
        ),
    }
}

fn emit_error(record: &LogRecord, rendered_text: Option<&str>) {
    match (record.target.as_deref(), rendered_text) {
        (Some(target), Some(text)) => tracing::event!(
            tracing::Level::ERROR,
            log_code = %record.code,
            log_target = %target,
            text = %text,
            fields = ?record.fields
        ),
        (Some(target), None) => tracing::event!(
            tracing::Level::ERROR,
            log_code = %record.code,
            log_target = %target,
            fields = ?record.fields
        ),
        (None, Some(text)) => tracing::event!(
            tracing::Level::ERROR,
            log_code = %record.code,
            text = %text,
            fields = ?record.fields
        ),
        (None, None) => tracing::event!(
            tracing::Level::ERROR,
            log_code = %record.code,
            fields = ?record.fields
        ),
    }
}

impl Display for LogRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.level.as_str(), self.code)?;

        if let Some(target) = &self.target {
            write!(f, " @{target}")?;
        }

        if let Some(text) = &self.text {
            write!(f, ": {}", text.diagnostic_display())?;
        }

        if self.fields.is_empty() {
            return Ok(());
        }

        f.write_str(" {")?;
        for (index, (name, value)) in self.fields.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{name}={value}")?;
        }
        f.write_str("}")
    }
}
