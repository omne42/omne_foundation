use std::collections::{BTreeSet, HashMap};

use crate::Event;
use crate::sinks::markdown::{
    Inline as MarkdownInline, Line as MarkdownLine, parse_markdown_lines,
};
use crate::sinks::text::{TextLimits, format_event_text_limited, truncate_chars};

use super::FeishuWebhookSink;

impl FeishuWebhookSink {
    pub(super) fn base_payload(
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut obj = serde_json::Map::with_capacity(6);
        if let Some(timestamp) = timestamp {
            obj.insert("timestamp".to_string(), serde_json::json!(timestamp));
        }
        if let Some(sign) = sign {
            obj.insert("sign".to_string(), serde_json::json!(sign));
        }
        obj
    }

    pub(super) fn build_text_payload(
        event: &Event,
        max_chars: usize,
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> serde_json::Value {
        let mut obj = Self::base_payload(timestamp, sign);
        obj.insert("msg_type".to_string(), serde_json::json!("text"));
        obj.insert(
            "content".to_string(),
            serde_json::json!({
                "text": format_event_text_limited(event, TextLimits::new(max_chars)),
            }),
        );
        serde_json::Value::Object(obj)
    }

    pub(super) async fn build_payload(
        &self,
        event: &Event,
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> crate::Result<serde_json::Value> {
        if !self.enable_markdown_rich_text {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        }

        let Some(body) = event
            .body
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        };

        let markdown_lines = parse_markdown_lines(body);
        if markdown_lines.is_empty() {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        }

        let image_keys = self.resolve_image_keys(&markdown_lines).await;

        let mut content_rows: Vec<serde_json::Value> = Vec::new();
        let mut remaining = self.max_chars;

        for line in markdown_lines {
            let mut row: Vec<serde_json::Value> = Vec::new();
            for inline in line.inlines {
                match inline {
                    MarkdownInline::Text(text) => {
                        let text = Self::take_text_budget(&text, &mut remaining);
                        if text.is_empty() {
                            continue;
                        }
                        row.push(serde_json::json!({
                            "tag": "text",
                            "text": text,
                        }));
                    }
                    MarkdownInline::Link { text, href } => {
                        let display = if text.trim().is_empty() {
                            href.clone()
                        } else {
                            text
                        };
                        let display = Self::take_text_budget(&display, &mut remaining);
                        if display.is_empty() {
                            continue;
                        }
                        row.push(serde_json::json!({
                            "tag": "a",
                            "text": display,
                            "href": href,
                        }));
                    }
                    MarkdownInline::Image { alt, src } => {
                        if let Some(image_key) =
                            image_keys.get(&src).and_then(|value| value.clone())
                        {
                            row.push(serde_json::json!({
                                "tag": "img",
                                "image_key": image_key,
                            }));
                            continue;
                        }

                        let fallback = if alt.trim().is_empty() {
                            format!("[image] {src}")
                        } else {
                            format!("[image:{alt}] {src}")
                        };
                        let fallback = Self::take_text_budget(&fallback, &mut remaining);
                        if fallback.is_empty() {
                            continue;
                        }
                        row.push(serde_json::json!({
                            "tag": "text",
                            "text": fallback,
                        }));
                    }
                }
            }

            if !row.is_empty() {
                content_rows.push(serde_json::Value::Array(row));
            }
            if remaining == 0 {
                break;
            }
        }

        for (key, value) in &event.tags {
            if remaining == 0 {
                break;
            }
            let tag_line = format!("{key}={value}");
            let text = Self::take_text_budget(&tag_line, &mut remaining);
            if text.is_empty() {
                break;
            }
            content_rows.push(serde_json::json!([
                {
                    "tag": "text",
                    "text": text,
                }
            ]));
        }

        if content_rows.is_empty() {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        }

        let title = truncate_chars(event.title.trim(), 256);
        let mut obj = Self::base_payload(timestamp, sign);
        obj.insert("msg_type".to_string(), serde_json::json!("post"));
        obj.insert(
            "content".to_string(),
            serde_json::json!({
                "post": {
                    "zh_cn": {
                        "title": title,
                        "content": content_rows,
                    }
                }
            }),
        );

        Ok(serde_json::Value::Object(obj))
    }

    pub(super) fn take_text_budget(input: &str, remaining: &mut usize) -> String {
        if *remaining == 0 || input.is_empty() {
            return String::new();
        }

        let taken = truncate_chars(input, *remaining);
        let taken_chars = taken.chars().count();
        if taken_chars >= *remaining {
            *remaining = 0;
        } else {
            *remaining -= taken_chars;
        }
        taken
    }

    pub(super) async fn resolve_image_keys(
        &self,
        markdown_lines: &[MarkdownLine],
    ) -> HashMap<String, Option<String>> {
        let mut urls = BTreeSet::new();
        for line in markdown_lines {
            for inline in &line.inlines {
                if let MarkdownInline::Image { src, .. } = inline {
                    urls.insert(src.clone());
                }
            }
        }

        let mut out = HashMap::with_capacity(urls.len());
        for src in urls {
            let key = self.resolve_single_image_key(&src).await;
            out.insert(src, key);
        }
        out
    }
}
