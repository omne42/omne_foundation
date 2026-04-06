use std::borrow::Cow;
use std::collections::BTreeMap;

use structured_text_kit::StructuredText;

fn plain_fallback(text: &StructuredText) -> Option<String> {
    text.freeform_text().map(ToOwned::to_owned)
}

fn render_text<'a>(plain: Option<&'a str>, text: &'a StructuredText) -> Cow<'a, str> {
    plain
        .map(Cow::Borrowed)
        .or_else(|| text.freeform_text().map(Cow::Borrowed))
        .unwrap_or(Cow::Borrowed(""))
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
    title: Option<String>,
    title_text: StructuredText,
    body: Option<String>,
    body_text: Option<StructuredText>,
    tags: BTreeMap<String, String>,
    tag_texts: BTreeMap<String, StructuredText>,
}

impl Event {
    pub fn new(kind: impl Into<String>, severity: Severity, title: impl Into<String>) -> Self {
        Self::new_structured(kind, severity, StructuredText::freeform(title.into()))
    }

    pub fn new_structured(
        kind: impl Into<String>,
        severity: Severity,
        title_text: StructuredText,
    ) -> Self {
        Self {
            kind: kind.into(),
            severity,
            title: plain_fallback(&title_text),
            title_text,
            body: None,
            body_text: None,
            tags: BTreeMap::new(),
            tag_texts: BTreeMap::new(),
        }
    }

    /// Renders the canonical title text into the plain-text view sinks consume.
    #[must_use]
    pub fn title(&self) -> Cow<'_, str> {
        render_text(self.title.as_deref(), &self.title_text)
    }

    #[must_use]
    pub fn title_text(&self) -> &StructuredText {
        &self.title_text
    }

    /// Renders the canonical body text into the plain-text view sinks consume.
    #[must_use]
    pub fn body(&self) -> Option<Cow<'_, str>> {
        self.body_text
            .as_ref()
            .map(|body_text| render_text(self.body.as_deref(), body_text))
    }

    #[must_use]
    pub fn body_text(&self) -> Option<&StructuredText> {
        self.body_text.as_ref()
    }

    /// Iterates rendered plain-text tags derived from the canonical structured tags.
    pub fn tags(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> + '_ {
        self.tag_texts.iter().map(|(key, value)| {
            (
                key.as_str(),
                render_text(self.tags.get(key).map(String::as_str), value),
            )
        })
    }

    #[must_use]
    pub fn tag_texts(&self) -> &BTreeMap<String, StructuredText> {
        &self.tag_texts
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        let title = title.into();
        self.title = Some(title.clone());
        self.title_text = StructuredText::freeform(title);
    }

    pub fn set_title_text(&mut self, title_text: StructuredText) {
        if let Some(title) = plain_fallback(&title_text) {
            self.title = Some(title);
        }
        self.title_text = title_text;
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        let body = body.into();
        self.body = Some(body.clone());
        self.body_text = Some(StructuredText::freeform(body));
    }

    pub fn clear_body(&mut self) {
        self.body = None;
        self.body_text = None;
    }

    pub fn set_body_text(&mut self, body_text: StructuredText) {
        if let Some(body) = plain_fallback(&body_text) {
            self.body = Some(body);
        }
        self.body_text = Some(body_text);
    }

    pub fn insert_tag(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.insert_tag_text(key, StructuredText::freeform(value.into()));
    }

    pub fn insert_tag_text(&mut self, key: impl Into<String>, value: StructuredText) {
        let key = key.into();
        if let Some(value_text) = plain_fallback(&value) {
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

    fn rendered_tags(event: &Event) -> BTreeMap<String, String> {
        event
            .tags()
            .map(|(key, value)| (key.to_string(), value.into_owned()))
            .collect()
    }

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
    fn structured_builders_do_not_render_catalog_text_through_plain_accessors() {
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
        assert_eq!(event.body(), Some(Cow::Borrowed("")));
        assert!(
            rendered_tags(&event)
                .get("thread_id")
                .is_some_and(String::is_empty)
        );
    }

    #[test]
    fn mutators_preserve_existing_plain_projection_for_catalog_text() {
        let title_text = structured_text!("notify.title", "repo" => "omne");
        let body_text = structured_text!("notify.body", "step" => "review");
        let tag_text = structured_text!("notify.tag", "value" => "thread");

        let mut event = Event::new("kind", Severity::Info, "plain title")
            .with_body("plain body")
            .with_tag("thread_id", "plain thread");
        event.set_title_text(title_text.clone());
        event.set_body_text(body_text.clone());
        event.insert_tag_text("thread_id", tag_text.clone());

        assert_eq!(event.title().as_ref(), "plain title");
        assert_eq!(event.body().as_deref(), Some("plain body"));
        assert_eq!(
            rendered_tags(&event).get("thread_id").map(String::as_str),
            Some("plain thread")
        );
        assert_eq!(event.title_text(), &title_text);
        assert_eq!(event.body_text(), Some(&body_text));
        assert_eq!(event.tag_texts().get("thread_id"), Some(&tag_text));
    }

    #[test]
    fn clear_body_and_remove_tag_drop_rendered_projection() {
        let mut event = Event::new("kind", Severity::Info, "title")
            .with_body("body")
            .with_tag("thread_id", "thread");

        event.clear_body();
        event.remove_tag("thread_id");

        assert_eq!(event.body(), None);
        assert_eq!(event.body_text(), None);
        assert_eq!(event.tags().count(), 0);
        assert!(!event.tag_texts().contains_key("thread_id"));
    }
}
