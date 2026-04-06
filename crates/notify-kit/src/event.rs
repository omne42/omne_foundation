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
    title: String,
    title_text: StructuredText,
    body: Option<String>,
    body_text: Option<StructuredText>,
    tags: BTreeMap<String, String>,
    tag_texts: BTreeMap<String, StructuredText>,
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
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn title_text(&self) -> &StructuredText {
        &self.title_text
    }

    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    #[must_use]
    pub fn body_text(&self) -> Option<&StructuredText> {
        self.body_text.as_ref()
    }

    #[must_use]
    pub fn tags(&self) -> &BTreeMap<String, String> {
        &self.tags
    }

    #[must_use]
    pub fn tag_texts(&self) -> &BTreeMap<String, StructuredText> {
        &self.tag_texts
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        let title = title.into();
        self.title_text = StructuredText::freeform(title.clone());
        self.title = title;
    }

    pub fn set_title_text(&mut self, title_text: StructuredText) {
        if let Some(title) = plain_text_projection(&title_text) {
            self.title = title;
        }
        self.title_text = title_text;
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        let body = body.into();
        self.body_text = Some(StructuredText::freeform(body.clone()));
        self.body = Some(body);
    }

    pub fn clear_body(&mut self) {
        self.body = None;
        self.body_text = None;
    }

    pub fn set_body_text(&mut self, body_text: StructuredText) {
        if let Some(body) = plain_text_projection(&body_text) {
            self.body = Some(body);
        }
        self.body_text = Some(body_text);
    }

    pub fn insert_tag(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        self.tag_texts
            .insert(key.clone(), StructuredText::freeform(value.clone()));
        self.tags.insert(key, value);
    }

    pub fn insert_tag_text(&mut self, key: impl Into<String>, value: StructuredText) {
        let key = key.into();
        if let Some(value_text) = plain_text_projection(&value) {
            self.tags.insert(key.clone(), value_text);
        }
        self.tag_texts.insert(key, value);
    }

    pub fn remove_tag(&mut self, key: &str) {
        self.tags.remove(key);
        self.tag_texts.remove(key);
    }

    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.set_body(body);
        self
    }

    #[must_use]
    pub fn with_title_text(mut self, title_text: StructuredText) -> Self {
        self.set_title_text(title_text);
        self
    }

    #[must_use]
    pub fn with_body_text(mut self, body_text: StructuredText) -> Self {
        self.set_body_text(body_text);
        self
    }

    #[must_use]
    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.insert_tag(key, value);
        self
    }

    #[must_use]
    pub fn with_tag_text(mut self, key: impl Into<String>, value: StructuredText) -> Self {
        self.insert_tag_text(key, value);
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

        assert_eq!(event.title_text().freeform_text(), Some("title"));
        assert_eq!(
            event.body_text().and_then(StructuredText::freeform_text),
            Some("body")
        );
        assert_eq!(
            event
                .tag_texts()
                .get("thread_id")
                .and_then(StructuredText::freeform_text),
            Some("t1")
        );
    }

    #[test]
    fn structured_builders_keep_catalog_text_without_synthesizing_plain_fallbacks() {
        let title = structured_text!("notify.title", "repo" => "omne");
        let body = structured_text!("notify.body", "step" => "review");
        let tag = structured_text!("notify.tag", "value" => "t1");

        let event = Event::new_structured("kind", Severity::Warning, title.clone())
            .with_body_text(body.clone())
            .with_tag_text("thread_id", tag.clone());

        assert_eq!(event.title_text(), &title);
        assert_eq!(event.body_text(), Some(&body));
        assert_eq!(event.tag_texts().get("thread_id"), Some(&tag));
        assert!(event.title().is_empty());
        assert_eq!(event.body(), None);
        assert_eq!(event.tags().get("thread_id"), None);
    }

    #[test]
    fn structured_freeform_builders_keep_plain_text_projection() {
        let title = StructuredText::freeform("title");
        let body = StructuredText::freeform("body");
        let tag = StructuredText::freeform("t1");

        let event = Event::new_structured("kind", Severity::Warning, title)
            .with_body_text(body)
            .with_tag_text("thread_id", tag);

        assert_eq!(event.title(), "title");
        assert_eq!(event.body(), Some("body"));
        assert_eq!(
            event.tags().get("thread_id").map(String::as_str),
            Some("t1")
        );
    }

    #[test]
    fn catalog_text_preserves_existing_plain_fallback() {
        let event = Event::new("kind", Severity::Info, "plain title")
            .with_body("plain body")
            .with_tag("thread_id", "plain-tag")
            .with_title_text(structured_text!("notify.title", "repo" => "omne"))
            .with_body_text(structured_text!("notify.body", "step" => "review"))
            .with_tag_text(
                "thread_id",
                structured_text!("notify.tag", "value" => "fresh"),
            );

        assert_eq!(event.title(), "plain title");
        assert_eq!(event.body(), Some("plain body"));
        assert_eq!(
            event.tags().get("thread_id").map(String::as_str),
            Some("plain-tag")
        );
    }

    #[test]
    fn mutators_keep_plain_and_structured_fields_in_sync() {
        let mut event = Event::new("kind", Severity::Info, "title");
        let title_text = structured_text!("notify.title", "repo" => "omne");
        let body_text = structured_text!("notify.body", "step" => "review");
        let tag_text = structured_text!("notify.tag", "value" => "thread");

        event.set_title("fresh title");
        event.set_title_text(title_text.clone());
        event.set_body("fresh body");
        event.set_body_text(body_text.clone());
        event.insert_tag("thread_id", "plain");
        event.insert_tag_text("thread_id", tag_text.clone());
        event.remove_tag("thread_id");
        event.clear_body();

        assert_eq!(event.title(), "fresh title");
        assert_eq!(event.title_text(), &title_text);
        assert_eq!(event.body(), None);
        assert_eq!(event.body_text(), None);
        assert!(!event.tags().contains_key("thread_id"));
        assert!(!event.tag_texts().contains_key("thread_id"));
    }
}
