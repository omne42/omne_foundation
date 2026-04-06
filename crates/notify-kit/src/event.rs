use std::borrow::Cow;
use std::collections::BTreeMap;

use structured_text_kit::StructuredText;

fn render_text(text: &StructuredText) -> Cow<'_, str> {
    match text.freeform_text() {
        Some(text) => Cow::Borrowed(text),
        None => Cow::Owned(text.to_string()),
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
    title_text: StructuredText,
    body_text: Option<StructuredText>,
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
            title_text,
            body_text: None,
            tag_texts: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn title(&self) -> Cow<'_, str> {
        render_text(&self.title_text)
    }

    #[must_use]
    pub fn title_text(&self) -> &StructuredText {
        &self.title_text
    }

    #[must_use]
    pub fn body(&self) -> Option<Cow<'_, str>> {
        self.body_text.as_ref().map(render_text)
    }

    #[must_use]
    pub fn body_text(&self) -> Option<&StructuredText> {
        self.body_text.as_ref()
    }

    pub fn tags(&self) -> impl Iterator<Item = (&str, Cow<'_, str>)> + '_ {
        self.tag_texts
            .iter()
            .map(|(key, value)| (key.as_str(), render_text(value)))
    }

    #[must_use]
    pub fn tag_texts(&self) -> &BTreeMap<String, StructuredText> {
        &self.tag_texts
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title_text = StructuredText::freeform(title.into());
    }

    pub fn set_title_text(&mut self, title_text: StructuredText) {
        self.title_text = title_text;
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        self.body_text = Some(StructuredText::freeform(body.into()));
    }

    pub fn clear_body(&mut self) {
        self.body_text = None;
    }

    pub fn set_body_text(&mut self, body_text: StructuredText) {
        self.body_text = Some(body_text);
    }

    pub fn insert_tag(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.insert_tag_text(key, StructuredText::freeform(value.into()));
    }

    pub fn insert_tag_text(&mut self, key: impl Into<String>, value: StructuredText) {
        self.tag_texts.insert(key.into(), value);
    }

    pub fn remove_tag(&mut self, key: &str) {
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
    fn structured_builders_render_catalog_text_through_plain_accessors() {
        let title = structured_text!("notify.title", "repo" => "omne");
        let body = structured_text!("notify.body", "step" => "review");
        let tag = structured_text!("notify.tag", "value" => "t1");

        let event = Event::new_structured("kind", Severity::Warning, title.clone())
            .with_body_text(body.clone())
            .with_tag_text("thread_id", tag.clone());

        let rendered_title = title.to_string();
        let rendered_body = body.to_string();
        let rendered_tag = tag.to_string();
        assert_eq!(event.title_text(), &title);
        assert_eq!(event.body_text(), Some(&body));
        assert_eq!(event.tag_texts().get("thread_id"), Some(&tag));
        assert_eq!(event.title().as_ref(), rendered_title.as_str());
        assert_eq!(event.body().as_deref(), Some(rendered_body.as_str()));
        assert_eq!(
            rendered_tags(&event).get("thread_id").map(String::as_str),
            Some(rendered_tag.as_str())
        );
    }

    #[test]
    fn mutators_replace_rendered_projection_with_latest_structured_value() {
        let title_text = structured_text!("notify.title", "repo" => "omne");
        let body_text = structured_text!("notify.body", "step" => "review");
        let tag_text = structured_text!("notify.tag", "value" => "thread");

        let mut event = Event::new("kind", Severity::Info, "plain title");
        event.set_title_text(title_text.clone());
        event.set_body_text(body_text.clone());
        event.insert_tag_text("thread_id", tag_text.clone());

        let rendered_title = title_text.to_string();
        let rendered_body = body_text.to_string();
        let rendered_tag = tag_text.to_string();
        assert_eq!(event.title().as_ref(), rendered_title.as_str());
        assert_eq!(event.body().as_deref(), Some(rendered_body.as_str()));
        assert_eq!(
            rendered_tags(&event).get("thread_id").map(String::as_str),
            Some(rendered_tag.as_str())
        );
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
