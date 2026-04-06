use std::collections::BTreeMap;

use structured_text_kit::StructuredText;

fn plain_text_projection(text: &StructuredText) -> Option<String> {
    text.freeform_text().map(ToString::to_string)
}

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
    pub title_text: StructuredText,
    pub body: Option<String>,
    pub body_text: Option<StructuredText>,
    pub tags: BTreeMap<String, String>,
    pub tag_texts: BTreeMap<String, StructuredText>,
}

impl Event {
    pub fn new(kind: impl Into<String>, severity: Severity, title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            kind: kind.into(),
            severity,
            title_text: StructuredText::freeform(title.clone()),
            title,
            body: None,
            body_text: None,
            tags: BTreeMap::new(),
            tag_texts: BTreeMap::new(),
        }
    }

    pub fn new_structured(
        kind: impl Into<String>,
        severity: Severity,
        title_text: StructuredText,
    ) -> Self {
        Self {
            kind: kind.into(),
            severity,
            title: plain_text_projection(&title_text).unwrap_or_default(),
            title_text,
            body: None,
            body_text: None,
            tags: BTreeMap::new(),
            tag_texts: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        let body = body.into();
        self.body_text = Some(StructuredText::freeform(body.clone()));
        self.body = Some(body);
        self
    }

    #[must_use]
    pub fn with_title_text(mut self, title_text: StructuredText) -> Self {
        self.title = plain_text_projection(&title_text).unwrap_or_default();
        self.title_text = title_text;
        self
    }

    #[must_use]
    pub fn with_body_text(mut self, body_text: StructuredText) -> Self {
        self.body = plain_text_projection(&body_text);
        self.body_text = Some(body_text);
        self
    }

    #[must_use]
    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        let value = value.into();
        self.tag_texts
            .insert(key.clone(), StructuredText::freeform(value.clone()));
        self.tags.insert(key, value);
        self
    }

    #[must_use]
    pub fn with_tag_text(mut self, key: impl Into<String>, value: StructuredText) -> Self {
        let key = key.into();
        if let Some(value_text) = plain_text_projection(&value) {
            self.tags.insert(key.clone(), value_text);
        } else {
            self.tags.remove(&key);
        }
        self.tag_texts.insert(key, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structured_text_kit::structured_text;

    #[test]
    fn string_builders_keep_freeform_structured_text_in_sync() {
        let event = Event::new("kind", Severity::Info, "title")
            .with_body("body")
            .with_tag("thread_id", "t1");

        assert_eq!(event.title_text.freeform_text(), Some("title"));
        assert_eq!(
            event
                .body_text
                .as_ref()
                .and_then(StructuredText::freeform_text),
            Some("body")
        );
        assert_eq!(
            event
                .tag_texts
                .get("thread_id")
                .and_then(StructuredText::freeform_text),
            Some("t1")
        );
    }

    #[test]
    fn structured_builders_preserve_catalog_text_without_breaking_string_fields() {
        let title = structured_text!("notify.title", "repo" => "omne");
        let body = structured_text!("notify.body", "step" => "review");
        let tag = structured_text!("notify.tag", "value" => "t1");

        let event = Event::new_structured("kind", Severity::Warning, title.clone())
            .with_body_text(body.clone())
            .with_tag_text("thread_id", tag.clone());

        assert_eq!(event.title_text, title);
        assert_eq!(event.body_text, Some(body));
        assert_eq!(event.tag_texts.get("thread_id"), Some(&tag));
        assert!(event.title.is_empty());
        assert_eq!(event.body, None);
        assert_eq!(event.tags.get("thread_id"), None);
    }

    #[test]
    fn structured_freeform_builders_keep_plain_text_projection() {
        let title = StructuredText::freeform("title");
        let body = StructuredText::freeform("body");
        let tag = StructuredText::freeform("t1");

        let event = Event::new_structured("kind", Severity::Warning, title)
            .with_body_text(body)
            .with_tag_text("thread_id", tag);

        assert_eq!(event.title, "title");
        assert_eq!(event.body.as_deref(), Some("body"));
        assert_eq!(event.tags.get("thread_id").map(String::as_str), Some("t1"));
    }
}
