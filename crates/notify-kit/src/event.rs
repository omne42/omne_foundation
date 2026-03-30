use std::borrow::Cow;
use std::collections::BTreeMap;

use structured_text_kit::StructuredText;

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
    title: StructuredText,
    body: Option<StructuredText>,
    tags: BTreeMap<String, StructuredText>,
}

impl Event {
    pub fn new(kind: impl Into<String>, severity: Severity, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            severity,
            title: StructuredText::freeform(title.into()),
            body: None,
            tags: BTreeMap::new(),
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
            title: title_text,
            body: None,
            tags: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(StructuredText::freeform(body.into()));
        self
    }

    #[must_use]
    pub fn with_title_text(mut self, title_text: StructuredText) -> Self {
        self.title = title_text;
        self
    }

    #[must_use]
    pub fn with_body_text(mut self, body_text: StructuredText) -> Self {
        self.body = Some(body_text);
        self
    }

    #[must_use]
    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags
            .insert(key.into(), StructuredText::freeform(value.into()));
        self
    }

    #[must_use]
    pub fn with_tag_text(mut self, key: impl Into<String>, value: StructuredText) -> Self {
        self.tags.insert(key.into(), value);
        self
    }

    pub fn title(&self) -> Cow<'_, str> {
        structured_text_to_cow(&self.title)
    }

    pub fn title_text(&self) -> &StructuredText {
        &self.title
    }

    pub fn body(&self) -> Option<Cow<'_, str>> {
        self.body.as_ref().map(structured_text_to_cow)
    }

    pub fn body_text(&self) -> Option<&StructuredText> {
        self.body.as_ref()
    }

    pub fn tags(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> + '_ {
        self.tags
            .iter()
            .map(|(key, value)| (key.as_str(), structured_text_to_cow(value)))
    }

    pub fn tag(&self, key: &str) -> Option<Cow<'_, str>> {
        self.tags.get(key).map(structured_text_to_cow)
    }

    pub fn tag_text(&self, key: &str) -> Option<&StructuredText> {
        self.tags.get(key)
    }

    pub fn tag_texts(&self) -> &BTreeMap<String, StructuredText> {
        &self.tags
    }
}

fn structured_text_to_cow(value: &StructuredText) -> Cow<'_, str> {
    match value.freeform_text() {
        Some(text) => Cow::Borrowed(text),
        None => Cow::Owned(value.to_string()),
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
                .tag_text("thread_id")
                .and_then(StructuredText::freeform_text),
            Some("t1")
        );
        assert_eq!(event.title().as_ref(), "title");
        assert_eq!(event.body().as_deref(), Some("body"));
        assert_eq!(event.tag("thread_id").as_deref(), Some("t1"));
    }

    #[test]
    fn structured_builders_preserve_catalog_text_without_breaking_string_fields() {
        let title = structured_text!("notify.title", "repo" => "omne");
        let body = structured_text!("notify.body", "step" => "review");
        let tag = structured_text!("notify.tag", "value" => "t1");

        let event = Event::new_structured("kind", Severity::Warning, title.clone())
            .with_body_text(body.clone())
            .with_tag_text("thread_id", tag.clone());

        assert_eq!(event.title_text(), &title);
        assert_eq!(event.body_text(), Some(&body));
        assert_eq!(event.tag_text("thread_id"), Some(&tag));
        assert_eq!(event.title().into_owned(), title.to_string());
        assert_eq!(event.body().map(Cow::into_owned), Some(body.to_string()));
        assert_eq!(
            event.tag("thread_id").map(Cow::into_owned),
            Some(tag.to_string())
        );
    }
}
