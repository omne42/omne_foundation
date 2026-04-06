use std::borrow::Cow;

use crate::Event;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TextLimits {
    pub max_chars: usize,
    pub max_title_chars: usize,
    pub max_body_chars: usize,
    pub max_tags: usize,
    pub max_tag_key_chars: usize,
    pub max_tag_value_chars: usize,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            max_chars: 16 * 1024,
            max_title_chars: 256,
            max_body_chars: 4 * 1024,
            max_tags: 32,
            max_tag_key_chars: 64,
            max_tag_value_chars: 256,
        }
    }
}

impl TextLimits {
    pub(crate) fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            ..Self::default()
        }
    }
}

struct LimitedChars {
    max: usize,
    out: String,
    out_chars: usize,
    truncated: bool,
}

fn byte_index_after_n_chars(input: &str, max_chars: usize) -> usize {
    if max_chars == 0 {
        return 0;
    }
    if input.is_ascii() {
        return input.len().min(max_chars);
    }
    let mut count = 0usize;
    for (idx, ch) in input.char_indices() {
        count += 1;
        if count == max_chars {
            return idx + ch.len_utf8();
        }
    }
    input.len()
}

fn take_prefix_chars(input: &str, max_chars: usize) -> (&str, usize, bool) {
    if max_chars == 0 {
        return ("", 0, !input.is_empty());
    }
    if input.is_ascii() {
        let end = input.len().min(max_chars);
        return (&input[..end], end, input.len() > max_chars);
    }
    let mut count = 0usize;
    for (idx, _) in input.char_indices() {
        if count == max_chars {
            return (&input[..idx], count, true);
        }
        count += 1;
    }
    (input, count, false)
}

impl LimitedChars {
    fn new(max: usize) -> Self {
        Self {
            max,
            out: String::with_capacity(max.min(256)),
            out_chars: 0,
            truncated: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    fn is_full(&self) -> bool {
        self.truncated || self.max == 0 || self.out_chars >= self.max
    }

    fn remaining_chars(&self) -> usize {
        self.max.saturating_sub(self.out_chars)
    }

    fn push_char(&mut self, ch: char) {
        if self.truncated || self.max == 0 {
            return;
        }
        if self.out_chars >= self.max {
            self.truncated = true;
            return;
        }
        self.out.push(ch);
        self.out_chars += 1;
    }

    fn push_str(&mut self, s: &str) {
        if self.truncated || self.max == 0 {
            return;
        }
        let remaining = self.max.saturating_sub(self.out_chars);
        if remaining == 0 {
            self.truncated = true;
            return;
        }
        let (prefix, chars_taken, was_truncated) = take_prefix_chars(s, remaining);
        self.out.push_str(prefix);
        self.out_chars += chars_taken;
        self.truncated = was_truncated;
    }

    fn finish(mut self) -> String {
        if self.truncated && self.max > 3 {
            let keep = self.max - 3;
            let keep_end = byte_index_after_n_chars(&self.out, keep);
            self.out.truncate(keep_end);
            self.out.push_str("...");
        }
        self.out
    }
}

fn format_event_text_parts_limited(
    event: &Event,
    limits: TextLimits,
    include_title: bool,
) -> String {
    let mut out = LimitedChars::new(limits.max_chars);
    if out.is_full() {
        return out.finish();
    }

    if include_title {
        let title = truncate_chars_cow(&event.title, limits.max_title_chars);
        out.push_str(title.as_ref());
        if out.is_full() {
            return out.finish();
        }
    }

    if let Some(body) = event.body.as_deref() {
        let body = body.trim();
        if !body.is_empty() {
            if !out.is_empty() {
                if out.remaining_chars() <= 1 {
                    out.truncated = true;
                    return out.finish();
                }
                out.push_char('\n');
            }
            if out.is_full() {
                return out.finish();
            }
            let body = truncate_chars_cow(body, limits.max_body_chars);
            out.push_str(body.as_ref());
            if out.is_full() {
                return out.finish();
            }
        }
    }

    for (idx, (k, v)) in event.tags.iter().enumerate() {
        if idx >= limits.max_tags || out.is_full() {
            break;
        }
        if !out.is_empty() {
            if out.remaining_chars() <= 1 {
                out.truncated = true;
                break;
            }
            out.push_char('\n');
        }
        if out.is_full() {
            break;
        }
        let key = truncate_chars_cow(k, limits.max_tag_key_chars);
        out.push_str(key.as_ref());
        if out.is_full() {
            break;
        }
        out.push_char('=');
        if out.is_full() {
            break;
        }
        let value = truncate_chars_cow(v, limits.max_tag_value_chars);
        out.push_str(value.as_ref());
    }

    out.finish()
}

pub(crate) fn format_event_text_limited(event: &Event, limits: TextLimits) -> String {
    format_event_text_parts_limited(event, limits, true)
}

pub(crate) fn format_event_body_and_tags_limited(event: &Event, limits: TextLimits) -> String {
    format_event_text_parts_limited(event, limits, false)
}

fn truncate_chars_cow(input: &str, max_chars: usize) -> Cow<'_, str> {
    if max_chars == 0 {
        return Cow::Borrowed("");
    }
    if input.is_ascii() {
        if input.len() <= max_chars {
            return Cow::Borrowed(input);
        }
        if max_chars > 3 {
            let keep = max_chars - 3;
            let mut out = String::with_capacity(max_chars);
            out.push_str(&input[..keep]);
            out.push_str("...");
            return Cow::Owned(out);
        }
        return Cow::Borrowed(&input[..max_chars]);
    }

    let keep_chars_for_ellipsis = max_chars.saturating_sub(3);
    let mut seen = 0usize;
    let mut end = input.len();
    let mut keep_end = 0usize;
    let mut truncated = false;

    for (idx, ch) in input.char_indices() {
        seen += 1;
        let next = idx + ch.len_utf8();
        if max_chars > 3 && seen == keep_chars_for_ellipsis {
            keep_end = next;
        }
        if seen == max_chars {
            end = next;
            truncated = next < input.len();
            break;
        }
    }

    if !truncated {
        if seen < max_chars {
            return Cow::Borrowed(input);
        }
        if end == input.len() {
            return Cow::Borrowed(input);
        }
    }

    if max_chars > 3 {
        let mut out = String::with_capacity(keep_end + 3);
        out.push_str(&input[..keep_end]);
        out.push_str("...");
        return Cow::Owned(out);
    }

    Cow::Borrowed(&input[..end])
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    truncate_chars_cow(input, max_chars).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn truncate_chars_is_utf8_safe() {
        let input = "a😀b";
        let out = truncate_chars(input, 3);
        assert_eq!(out, "a😀b");

        let out = truncate_chars(input, 2);
        assert_eq!(out, "a😀");

        let out = truncate_chars(input, 1);
        assert_eq!(out, "a");
    }

    #[test]
    fn truncate_chars_adds_ellipsis() {
        let input = "abcdef";
        let out = truncate_chars(input, 5);
        assert_eq!(out, "ab...");
    }

    #[test]
    fn truncate_chars_ascii_small_budget_has_no_ellipsis() {
        let input = "abcdef";
        let out = truncate_chars(input, 3);
        assert_eq!(out, "abc");
    }

    #[test]
    fn truncate_chars_cow_borrows_when_not_truncated() {
        let input = "abc";
        let out = truncate_chars_cow(input, 10);
        assert!(matches!(out, std::borrow::Cow::Borrowed("abc")));
    }

    #[test]
    fn format_event_text_limited_caps_tags_and_length() {
        let mut event = Event::new("k", Severity::Info, "title").with_body("body");
        for i in 0..100 {
            event = event.with_tag(format!("k{i}"), "v");
        }

        let limits = TextLimits {
            max_chars: 20,
            max_tags: 2,
            ..TextLimits::default()
        };

        let out = format_event_text_limited(&event, limits);
        assert!(out.chars().count() <= 20, "{out}");
        assert!(out.contains("title"), "{out}");
    }

    #[test]
    fn format_event_text_limited_keeps_title_only_when_already_full() {
        let event = Event::new("k", Severity::Info, "hello world")
            .with_body("body")
            .with_tag("k", "v");

        let out = format_event_text_limited(
            &event,
            TextLimits {
                max_chars: 8,
                ..TextLimits::default()
            },
        );
        assert_eq!(out, "hello...");
        assert!(!out.contains('\n'), "{out}");
        assert!(!out.contains("body"), "{out}");
        assert!(!out.contains("k=v"), "{out}");
    }

    #[test]
    fn format_event_text_limited_zero_char_budget_returns_empty() {
        let event = Event::new("k", Severity::Info, "title")
            .with_body("body")
            .with_tag("k", "v");
        let out = format_event_text_limited(
            &event,
            TextLimits {
                max_chars: 0,
                ..TextLimits::default()
            },
        );
        assert!(out.is_empty(), "{out}");
    }

    #[test]
    fn format_event_text_limited_no_trailing_newline_when_body_cannot_fit() {
        let event = Event::new("k", Severity::Info, "a").with_body("body");
        let out = format_event_text_limited(
            &event,
            TextLimits {
                max_chars: 2,
                ..TextLimits::default()
            },
        );
        assert_eq!(out, "a");
    }

    #[test]
    fn format_event_text_limited_no_trailing_newline_when_tag_cannot_fit() {
        let event = Event::new("k", Severity::Info, "a").with_tag("k", "v");
        let out = format_event_text_limited(
            &event,
            TextLimits {
                max_chars: 2,
                ..TextLimits::default()
            },
        );
        assert_eq!(out, "a");
    }
}
