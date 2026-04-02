use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use structured_text_kit::StructuredText;
use tracing::callsite::Callsite;
use tracing::field::Value;
use tracing::subscriber::Interest;

use crate::{LogCode, LogFieldNameValidationError, LogValue};

const TRACING_EVENT_NAME: &str = "log_record";
const DEFAULT_TRACING_TARGET: &str = module_path!();
const INTEREST_UNSET: u8 = 0xFF;
const INTEREST_NEVER: u8 = 0;
const INTEREST_SOMETIMES: u8 = 1;
const INTEREST_ALWAYS: u8 = 2;

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
        let rendered_text = self
            .text
            .as_ref()
            .map(|text| text.diagnostic_display().to_string());
        emit_tracing_event(self, rendered_text.as_deref());
    }
}

fn emit_tracing_event(record: &LogRecord, rendered_text: Option<&str>) {
    let callsite = tracing_callsite(record, rendered_text.is_some());
    if !callsite.is_enabled() {
        return;
    }

    let metadata = callsite.metadata();
    let field_set = metadata.fields();
    let mut values = Vec::with_capacity(2 + record.fields.len());
    let mut event_values = Vec::with_capacity(2 + record.fields.len());
    event_values.push(EventValue::Str(record.code.as_str()));
    if let Some(text) = rendered_text {
        event_values.push(EventValue::Str(text));
    }
    for value in record.fields.values() {
        event_values.push(EventValue::from_log_value(value));
    }

    values.extend(event_values.iter().map(|value| Some(value.as_value())));
    let value_set = field_set.value_set_all(&values);
    let event = tracing::Event::new(metadata, &value_set);
    tracing::dispatcher::get_default(|dispatch| dispatch.event(&event));
}

fn tracing_callsite(record: &LogRecord, include_text: bool) -> &'static DynamicCallsite {
    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<CallsiteKey, &'static DynamicCallsite>>,
    > = OnceLock::new();

    let key = CallsiteKey {
        level: record.level.as_tracing_level(),
        target: record
            .target
            .as_deref()
            .unwrap_or(DEFAULT_TRACING_TARGET)
            .to_string(),
        field_names: tracing_field_names(record, include_text),
    };
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(callsite) = cache.get(&key) {
        return callsite;
    }

    let callsite = DynamicCallsite::new(
        key.level,
        leak_string(key.target.clone()),
        leak_field_names(key.field_names.clone()),
    );
    cache.insert(key, callsite);
    callsite
}

fn tracing_field_names(record: &LogRecord, include_text: bool) -> Vec<String> {
    let mut names = Vec::with_capacity(2 + record.fields.len());
    names.push("log_code".to_string());
    if include_text {
        names.push("text".to_string());
    }
    names.extend(record.fields.keys().cloned());
    names
}

fn leak_string(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn leak_field_names(field_names: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = field_names.into_iter().map(leak_string).collect();
    Box::leak(leaked.into_boxed_slice())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CallsiteKey {
    level: tracing::Level,
    target: String,
    field_names: Vec<String>,
}

struct DynamicCallsite {
    interest: AtomicU8,
    registered: AtomicBool,
    metadata: OnceLock<tracing::Metadata<'static>>,
}

impl DynamicCallsite {
    fn new(
        level: tracing::Level,
        target: &'static str,
        field_names: &'static [&'static str],
    ) -> &'static Self {
        let callsite = Box::leak(Box::new(Self {
            interest: AtomicU8::new(INTEREST_UNSET),
            registered: AtomicBool::new(false),
            metadata: OnceLock::new(),
        }));
        let metadata = tracing::Metadata::new(
            TRACING_EVENT_NAME,
            target,
            level,
            Some(file!()),
            Some(line!()),
            Some(module_path!()),
            tracing::field::FieldSet::new(field_names, tracing::callsite::Identifier(callsite)),
            tracing::metadata::Kind::EVENT,
        );
        let _ = callsite.metadata.set(metadata);
        callsite.ensure_registered();
        callsite
    }

    fn ensure_registered(&'static self) {
        if !self.registered.swap(true, Ordering::AcqRel) {
            tracing::callsite::register(self);
        }
    }

    fn is_enabled(&'static self) -> bool {
        self.ensure_registered();
        match self.interest.load(Ordering::Acquire) {
            INTEREST_NEVER => false,
            INTEREST_ALWAYS => true,
            _ => tracing::dispatcher::get_default(|dispatch| dispatch.enabled(self.metadata())),
        }
    }
}

impl Callsite for DynamicCallsite {
    fn set_interest(&self, interest: Interest) {
        let value = if interest.is_never() {
            INTEREST_NEVER
        } else if interest.is_always() {
            INTEREST_ALWAYS
        } else {
            INTEREST_SOMETIMES
        };
        self.interest.store(value, Ordering::Release);
    }

    fn metadata(&self) -> &tracing::Metadata<'_> {
        self.metadata
            .get()
            .expect("dynamic log callsite metadata should be initialized")
    }
}

enum EventValue<'a> {
    Str(&'a str),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    OwnedString(String),
}

impl<'a> EventValue<'a> {
    fn from_log_value(value: &'a LogValue) -> Self {
        match value {
            LogValue::Text(value) => Self::Str(value.as_str()),
            LogValue::Bool(value) => Self::Bool(*value),
            LogValue::Signed(value) => Self::Signed(*value),
            LogValue::Unsigned(value) => Self::Unsigned(*value),
            LogValue::StructuredText(value) => {
                Self::OwnedString(value.diagnostic_display().to_string())
            }
        }
    }

    fn as_value(&self) -> &dyn Value {
        match self {
            Self::Str(value) => value as &dyn Value,
            Self::Bool(value) => value as &dyn Value,
            Self::Signed(value) => value as &dyn Value,
            Self::Unsigned(value) => value as &dyn Value,
            Self::OwnedString(value) => value as &dyn Value,
        }
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
