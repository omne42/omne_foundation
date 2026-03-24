use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub kind: String,
    pub severity: Severity,
    pub title: String,
    pub body: Option<String>,
    pub tags: BTreeMap<String, String>,
}

impl Event {
    pub fn new(kind: impl Into<String>, severity: Severity, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            severity,
            title: title.into(),
            body: None,
            tags: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    #[must_use]
    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }
}
