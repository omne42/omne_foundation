use std::borrow::Cow;
use std::collections::BTreeMap;

use structured_text_kit::StructuredText;

fn derive_plain_fallback(text: &StructuredText) -> Option<String> {
    text.freeform_text().map(ToOwned::to_owned)
}

fn render_text<'a>(plain: Option<&'a str>, text: &'a StructuredText) -> Cow<'a, str> {
    plain
        .map(Cow::Borrowed)
        .or_else(|| text.freeform_text().map(Cow::Borrowed))
        .unwrap_or(Cow::Borrowed(""))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextProjection {
    plain_fallback: Option<String>,
    structured: StructuredText,
}

impl TextProjection {
    fn new(structured: StructuredText) -> Self {
        Self {
            plain_fallback: derive_plain_fallback(&structured),
            structured,
        }
    }

    fn render(&self) -> Cow<'_, str> {
        render_text(self.plain_fallback.as_deref(), &self.structured)
    }

    fn structured(&self) -> &StructuredText {
        &self.structured
    }

    fn set_plain(&mut self, plain: String) {
        self.plain_fallback = Some(plain.clone());
        self.structured = StructuredText::freeform(plain);
    }

    fn set_structured(&mut self, structured: StructuredText) {
        self.plain_fallback = derive_plain_fallback(&structured).or(self.plain_fallback.take());
        self.structured = structured;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct OptionalTextProjection {
    plain_fallback: Option<String>,
    structured: Option<StructuredText>,
}

impl OptionalTextProjection {
    fn render(&self) -> Option<Cow<'_, str>> {
        self.structured
            .as_ref()
            .map(|text| render_text(self.plain_fallback.as_deref(), text))
    }

    fn structured(&self) -> Option<&StructuredText> {
        self.structured.as_ref()
    }

    fn clear(&mut self) {
        self.plain_fallback = None;
        self.structured = None;
    }

    fn set_plain(&mut self, plain: String) {
        self.plain_fallback = Some(plain.clone());
        self.structured = Some(StructuredText::freeform(plain));
    }

    fn set_structured(&mut self, structured: StructuredText) {
        self.plain_fallback = derive_plain_fallback(&structured).or(self.plain_fallback.take());
        self.structured = Some(structured);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TagProjection {
    plain_fallbacks: BTreeMap<String, String>,
    structured: BTreeMap<String, StructuredText>,
}

impl TagProjection {
    fn render(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> + '_ {
        self.structured.iter().map(|(key, value)| {
            (
                key.as_str(),
                render_text(self.plain_fallbacks.get(key).map(String::as_str), value),
            )
        })
    }

    fn structured(&self) -> &BTreeMap<String, StructuredText> {
        &self.structured
    }

    fn insert_plain(&mut self, key: String, plain: String) {
        self.plain_fallbacks.insert(key.clone(), plain.clone());
        self.structured.insert(key, StructuredText::freeform(plain));
    }

    fn insert_structured(&mut self, key: String, value: StructuredText) {
        let plain_fallback =
            derive_plain_fallback(&value).or_else(|| self.plain_fallbacks.remove(&key));
        if let Some(plain_fallback) = plain_fallback {
            self.plain_fallbacks.insert(key.clone(), plain_fallback);
        }
        self.structured.insert(key, value);
    }

    fn remove(&mut self, key: &str) {
        self.plain_fallbacks.remove(key);
        self.structured.remove(key);
    }
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
    title: TextProjection,
    body: OptionalTextProjection,
    tags: TagProjection,
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
            title: TextProjection::new(title_text),
            body: OptionalTextProjection::default(),
            tags: TagProjection::default(),
        }
    }

    /// Renders the canonical title text into the plain-text view sinks consume.
    #[must_use]
    pub fn title(&self) -> Cow<'_, str> {
        self.title.render()
    }

    #[must_use]
    pub fn title_text(&self) -> &StructuredText {
        self.title.structured()
    }

    /// Renders the canonical body text into the plain-text view sinks consume.
    #[must_use]
    pub fn body(&self) -> Option<Cow<'_, str>> {
        self.body.render()
    }

    #[must_use]
    pub fn body_text(&self) -> Option<&StructuredText> {
        self.body.structured()
    }

    /// Iterates rendered plain-text tags derived from the canonical structured tags.
    pub fn tags(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> + '_ {
        self.tags.render()
    }

    #[must_use]
    pub fn tag_texts(&self) -> &BTreeMap<String, StructuredText> {
        self.tags.structured()
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title.set_plain(title.into());
    }

    pub fn set_title_text(&mut self, title_text: StructuredText) {
        self.title.set_structured(title_text);
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        self.body.set_plain(body.into());
    }

    pub fn clear_body(&mut self) {
        self.body.clear();
    }

    pub fn set_body_text(&mut self, body_text: StructuredText) {
        self.body.set_structured(body_text);
    }

    pub fn insert_tag(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.tags.insert_plain(key.into(), value.into());
    }

    pub fn insert_tag_text(&mut self, key: impl Into<String>, value: StructuredText) {
        self.tags.insert_structured(key.into(), value);
    }

    pub fn remove_tag(&mut self, key: &str) {
        self.tags.remove(key);
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
