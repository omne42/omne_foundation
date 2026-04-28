#![forbid(unsafe_code)]

use std::collections::HashSet;

use regex::{NoExpand, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

const MAX_REGEXES: usize = 128;
const MAX_REGEX_PATTERN_BYTES: usize = 4096;
const MAX_REGEX_COMPILED_SIZE_BYTES: usize = 1_000_000;
const MAX_REGEX_NEST_LIMIT: u32 = 128;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RedactionError {
    #[error("replacement must not be empty")]
    EmptyReplacement,
    #[error("replacement must not contain control characters")]
    ReplacementContainsControl,
    #[error("too many regex patterns ({count}; max {max})")]
    TooManyRegexes { count: usize, max: usize },
    #[error("regex pattern is too large ({bytes} bytes; max {max} bytes)")]
    RegexPatternTooLarge { bytes: usize, max: usize },
    #[error("invalid regex pattern {pattern:?}: {message}")]
    InvalidRegex { pattern: String, message: String },
    #[error("json pointer must start with '/': {pointer}")]
    InvalidJsonPointerPrefix { pointer: String },
    #[error("invalid json pointer segment {segment:?}: {message}")]
    InvalidJsonPointerSegment { segment: String, message: String },
    #[error("{name} must be a finite value between 0.0 and 1.0")]
    InvalidSampleRate { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionRules {
    #[serde(default = "default_replacement")]
    pub replacement: String,
    #[serde(default)]
    pub redact_key_names: Vec<String>,
    #[serde(default)]
    pub redact_query_params: Vec<String>,
    #[serde(default)]
    pub sanitize_query_in_keys: Vec<String>,
    #[serde(default)]
    pub redact_json_pointers: Vec<String>,
    #[serde(default)]
    pub redact_regexes: Vec<String>,
}

impl Default for RedactionRules {
    fn default() -> Self {
        Self {
            replacement: default_replacement(),
            redact_key_names: Vec::new(),
            redact_query_params: Vec::new(),
            sanitize_query_in_keys: Vec::new(),
            redact_json_pointers: Vec::new(),
            redact_regexes: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct Redactor {
    replacement: String,
    redact_key_names: HashSet<String>,
    redact_query_params: HashSet<String>,
    sanitize_query_in_keys: HashSet<String>,
    redact_json_pointers: Vec<Vec<String>>,
    redact_regexes: Vec<Regex>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RedactionContext {
    sanitize_query: bool,
}

impl Redactor {
    pub fn new(rules: &RedactionRules) -> Result<Self, RedactionError> {
        let replacement = rules.replacement.trim();
        if replacement.is_empty() {
            return Err(RedactionError::EmptyReplacement);
        }
        if replacement.chars().any(char::is_control) {
            return Err(RedactionError::ReplacementContainsControl);
        }
        if rules.redact_regexes.len() > MAX_REGEXES {
            return Err(RedactionError::TooManyRegexes {
                count: rules.redact_regexes.len(),
                max: MAX_REGEXES,
            });
        }

        let mut redact_json_pointers = Vec::new();
        for pointer in &rules.redact_json_pointers {
            let pointer = pointer.trim();
            if pointer.is_empty() {
                continue;
            }
            redact_json_pointers.push(parse_json_pointer(pointer)?);
        }

        let mut redact_regexes = Vec::new();
        for pattern in &rules.redact_regexes {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            if pattern.len() > MAX_REGEX_PATTERN_BYTES {
                return Err(RedactionError::RegexPatternTooLarge {
                    bytes: pattern.len(),
                    max: MAX_REGEX_PATTERN_BYTES,
                });
            }
            let regex = RegexBuilder::new(pattern)
                .size_limit(MAX_REGEX_COMPILED_SIZE_BYTES)
                .nest_limit(MAX_REGEX_NEST_LIMIT)
                .build()
                .map_err(|err| RedactionError::InvalidRegex {
                    pattern: summarize_pattern_for_error(pattern),
                    message: err.to_string(),
                })?;
            redact_regexes.push(regex);
        }

        Ok(Self {
            replacement: replacement.to_string(),
            redact_key_names: normalize_list(&rules.redact_key_names),
            redact_query_params: normalize_list(&rules.redact_query_params),
            sanitize_query_in_keys: normalize_list(&rules.sanitize_query_in_keys),
            redact_json_pointers,
            redact_regexes,
        })
    }

    #[must_use]
    pub fn redact_json_value(&self, mut value: Value) -> Value {
        self.redact_value_in_place(&mut value, RedactionContext::default());
        for pointer in &self.redact_json_pointers {
            redact_json_pointer_in_place(&mut value, pointer, &self.replacement);
        }
        value
    }

    #[must_use]
    pub fn redact_named_string(&self, key: &str, value: &str) -> String {
        let normalized_key = normalize_name(key);
        if self.redact_key_names.contains(&normalized_key) {
            return self.replacement.clone();
        }

        self.redact_string(
            value,
            RedactionContext {
                sanitize_query: self.sanitize_query_in_keys.contains(&normalized_key),
            },
        )
    }

    #[must_use]
    pub fn redact_prometheus_render(&self, rendered: &str) -> String {
        if rendered.is_empty() {
            return String::new();
        }

        let has_trailing_newline = rendered.ends_with('\n');
        let mut out = String::with_capacity(rendered.len());
        for line in rendered.lines() {
            out.push_str(&self.redact_prometheus_line(line));
            out.push('\n');
        }
        if !has_trailing_newline {
            let _ = out.pop();
        }
        out
    }

    fn redact_value_in_place(&self, value: &mut Value, ctx: RedactionContext) {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
            Value::String(value) => {
                *value = self.redact_string(value, ctx);
            }
            Value::Array(items) => {
                for item in items {
                    self.redact_value_in_place(item, ctx);
                }
            }
            Value::Object(map) => {
                for (key, value) in map {
                    let normalized_key = normalize_name(key);
                    if self.redact_key_names.contains(&normalized_key) {
                        *value = Value::String(self.replacement.clone());
                        continue;
                    }

                    let child_ctx = RedactionContext {
                        sanitize_query: ctx.sanitize_query
                            || self.sanitize_query_in_keys.contains(&normalized_key),
                    };
                    self.redact_value_in_place(value, child_ctx);
                }
            }
        }
    }

    fn redact_string(&self, value: &str, ctx: RedactionContext) -> String {
        let mut out = if ctx.sanitize_query {
            redact_query_string(&self.redact_query_params, &self.replacement, value)
        } else {
            value.to_string()
        };

        for regex in &self.redact_regexes {
            if regex.is_match(&out) {
                out = regex
                    .replace_all(&out, NoExpand(&self.replacement))
                    .to_string();
            }
        }

        out
    }

    fn redact_prometheus_line(&self, line: &str) -> String {
        if line.starts_with('#') {
            return line.to_string();
        }
        let Some(open_idx) = line.find('{') else {
            return line.to_string();
        };
        let Some(close_rel) = line[open_idx + 1..].find('}') else {
            return line.to_string();
        };
        let close_idx = open_idx + 1 + close_rel;
        let Some(redacted_labels) = self.redact_prometheus_labels(&line[open_idx + 1..close_idx])
        else {
            return line.to_string();
        };

        let mut out = String::with_capacity(line.len());
        out.push_str(&line[..open_idx + 1]);
        out.push_str(&redacted_labels);
        out.push_str(&line[close_idx..]);
        out
    }

    fn redact_prometheus_labels(&self, labels: &str) -> Option<String> {
        let mut out = String::with_capacity(labels.len());
        let bytes = labels.as_bytes();
        let mut idx = 0usize;

        while idx < bytes.len() {
            let name_start = idx;
            while idx < bytes.len() && bytes[idx] != b'=' {
                idx += 1;
            }
            if idx == bytes.len() || idx + 1 >= bytes.len() || bytes[idx + 1] != b'"' {
                return None;
            }
            let name = &labels[name_start..idx];
            idx += 2;

            let mut raw_value = String::new();
            let mut closed = false;
            while idx < bytes.len() {
                match bytes[idx] {
                    b'\\' => {
                        idx += 1;
                        if idx == bytes.len() {
                            return None;
                        }
                        match bytes[idx] {
                            b'\\' => raw_value.push('\\'),
                            b'"' => raw_value.push('"'),
                            b'n' => raw_value.push('\n'),
                            other => raw_value.push(char::from(other)),
                        }
                        idx += 1;
                    }
                    b'"' => {
                        idx += 1;
                        closed = true;
                        break;
                    }
                    byte => {
                        raw_value.push(char::from(byte));
                        idx += 1;
                    }
                }
            }
            if !closed {
                return None;
            }

            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(name);
            out.push_str("=\"");
            out.push_str(&escape_prometheus_label_value(
                &self.redact_named_string(name, &raw_value),
            ));
            out.push('"');

            if idx == bytes.len() {
                break;
            }
            if bytes[idx] != b',' {
                return None;
            }
            idx += 1;
        }

        Some(out)
    }
}

pub fn validate_sample_rate(name: &str, value: f64) -> Result<(), RedactionError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(RedactionError::InvalidSampleRate {
        name: name.to_string(),
    })
}

pub fn stable_sample_json_payload(sink: &str, payload: &Value, rate: f64) -> bool {
    if !rate.is_finite() || rate <= 0.0 {
        return false;
    }
    if rate >= 1.0 {
        return true;
    }

    let threshold = (rate * u64::MAX as f64).floor() as u64;
    sample_hash(sink, payload) <= threshold
}

fn default_replacement() -> String {
    "<redacted>".to_string()
}

fn normalize_list(values: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for value in values {
        let normalized = normalize_name(value);
        if normalized.is_empty() {
            continue;
        }
        out.insert(normalized);
    }
    out
}

fn normalize_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn redact_query_string(redact: &HashSet<String>, replacement: &str, url: &str) -> String {
    let Some((path, query_and_fragment)) = url.split_once('?') else {
        return url.to_string();
    };
    let (query, fragment) = query_and_fragment
        .split_once('#')
        .map_or((query_and_fragment, None), |(query, fragment)| {
            (query, Some(fragment))
        });

    let mut out = String::with_capacity(url.len());
    out.push_str(path);
    out.push('?');

    let mut first = true;
    for pair in query.split('&') {
        if !first {
            out.push('&');
        }
        first = false;

        let (key, _value) = pair.split_once('=').unwrap_or((pair, ""));
        let normalized_key = normalize_name(key);
        if redact.contains(&normalized_key) {
            out.push_str(key);
            out.push('=');
            out.push_str(replacement);
        } else {
            out.push_str(pair);
        }
    }

    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }

    out
}

fn parse_json_pointer(pointer: &str) -> Result<Vec<String>, RedactionError> {
    if !pointer.starts_with('/') {
        return Err(RedactionError::InvalidJsonPointerPrefix {
            pointer: pointer.to_string(),
        });
    }

    let mut segments = Vec::new();
    for raw_segment in pointer.split('/').skip(1) {
        segments.push(decode_json_pointer_segment(raw_segment).map_err(|message| {
            RedactionError::InvalidJsonPointerSegment {
                segment: raw_segment.to_string(),
                message: message.to_string(),
            }
        })?);
    }
    Ok(segments)
}

fn decode_json_pointer_segment(segment: &str) -> Result<String, &'static str> {
    if !segment.contains('~') {
        return Ok(segment.to_string());
    }

    let mut out = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            out.push(ch);
            continue;
        }

        let Some(next) = chars.next() else {
            return Err("dangling '~' escape");
        };
        match next {
            '0' => out.push('~'),
            '1' => out.push('/'),
            _ => return Err("invalid '~' escape (expected ~0 or ~1)"),
        }
    }
    Ok(out)
}

fn redact_json_pointer_in_place(value: &mut Value, pointer: &[String], replacement: &str) {
    if pointer.is_empty() {
        *value = Value::String(replacement.to_string());
        return;
    }

    let mut current = value;
    for segment in &pointer[..pointer.len().saturating_sub(1)] {
        match current {
            Value::Object(map) => {
                let Some(next) = map.get_mut(segment) else {
                    return;
                };
                current = next;
            }
            Value::Array(items) => {
                let Ok(idx) = segment.parse::<usize>() else {
                    return;
                };
                let Some(next) = items.get_mut(idx) else {
                    return;
                };
                current = next;
            }
            _ => return,
        }
    }

    let Some(last) = pointer.last() else {
        return;
    };
    match current {
        Value::Object(map) if map.contains_key(last) => {
            map.insert(last.clone(), Value::String(replacement.to_string()));
        }
        Value::Array(items) => {
            if let Ok(idx) = last.parse::<usize>()
                && idx < items.len()
            {
                items[idx] = Value::String(replacement.to_string());
            }
        }
        _ => {}
    }
}

fn sample_hash(sink: &str, payload: &Value) -> u64 {
    let mut hash = fnv1a64_init();
    hash = fnv1a64_update(hash, sink.as_bytes());
    hash = fnv1a64_update(hash, b"|");

    if let Some(identity) = find_sampling_identity(payload) {
        return fnv1a64_update(hash, identity.as_bytes());
    }

    match serde_json::to_vec(payload) {
        Ok(bytes) => fnv1a64_update(hash, &bytes),
        Err(_) => hash,
    }
}

fn find_sampling_identity(value: &Value) -> Option<String> {
    const PREFERRED_KEYS: &[&str] = &["request_id", "trace_id", "response_id", "session_id", "id"];

    match value {
        Value::Object(map) => find_sampling_identity_in_object(map, PREFERRED_KEYS),
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_sampling_identity(item) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_sampling_identity_in_object(
    map: &Map<String, Value>,
    preferred_keys: &[&str],
) -> Option<String> {
    for key in preferred_keys {
        let Some(value) = map.get(*key) else {
            continue;
        };
        let Some(value) = value.as_str() else {
            continue;
        };
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    for value in map.values() {
        if let Some(found) = find_sampling_identity(value) {
            return Some(found);
        }
    }

    None
}

const FNV1A64_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV1A64_PRIME: u64 = 0x100000001b3;

fn fnv1a64_init() -> u64 {
    FNV1A64_OFFSET_BASIS
}

fn fnv1a64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV1A64_PRIME);
    }
    hash
}

fn escape_prometheus_label_value(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

fn summarize_pattern_for_error(pattern: &str) -> String {
    const MAX_BYTES: usize = 200;
    if pattern.len() <= MAX_BYTES {
        return pattern.to_string();
    }
    let mut end = MAX_BYTES;
    while end > 0 && !pattern.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}...", &pattern[..end])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{RedactionRules, Redactor, stable_sample_json_payload, validate_sample_rate};

    fn test_redactor() -> Redactor {
        Redactor::new(&RedactionRules {
            redact_key_names: vec!["authorization".to_string(), "token".to_string()],
            redact_query_params: vec!["api_key".to_string(), "token".to_string()],
            sanitize_query_in_keys: vec!["path".to_string(), "url".to_string()],
            redact_json_pointers: vec!["/a/b/0/c".to_string()],
            redact_regexes: vec![
                "(?i)bearer\\s+[^\\s]+".to_string(),
                "sk-[A-Za-z0-9]{10,}".to_string(),
            ],
            ..RedactionRules::default()
        })
        .expect("build redactor")
    }

    #[test]
    fn redacts_json_values_by_key_pointer_and_regex() {
        let redactor = test_redactor();
        let out = redactor.redact_json_value(json!({
            "authorization": "Bearer sk-test-secret",
            "nested": { "token": "sk-1234567890", "safe": "ok" },
            "a": { "b": [{ "c": "secret", "d": "keep" }] }
        }));

        assert_eq!(out["authorization"].as_str(), Some("<redacted>"));
        assert_eq!(out["nested"]["token"].as_str(), Some("<redacted>"));
        assert_eq!(out["nested"]["safe"].as_str(), Some("ok"));
        assert_eq!(out["a"]["b"][0]["c"].as_str(), Some("<redacted>"));
    }

    #[test]
    fn redacts_query_params_only_for_configured_names() {
        let redactor = test_redactor();
        assert_eq!(
            redactor.redact_named_string("path", "/v1?api_key=abc&x=1#frag"),
            "/v1?api_key=<redacted>&x=1#frag"
        );
        assert_eq!(
            redactor.redact_named_string("other", "/v1?api_key=abc&x=1"),
            "/v1?api_key=abc&x=1"
        );
    }

    #[test]
    fn redacts_prometheus_label_values() {
        let redactor = test_redactor();
        let rendered = "metric{token=\"abc\",path=\"/v1?api_key=abc\"} 1\n";
        assert_eq!(
            redactor.redact_prometheus_render(rendered),
            "metric{token=\"<redacted>\",path=\"/v1?api_key=<redacted>\"} 1\n"
        );
    }

    #[test]
    fn sample_rate_validation_and_sampling_are_stable() {
        validate_sample_rate("logs", 0.5).expect("valid rate");
        assert!(validate_sample_rate("logs", 1.5).is_err());
        assert!(!stable_sample_json_payload(
            "json_logs",
            &json!({"request_id": "req-1"}),
            f64::NAN
        ));

        let payload_a = json!({"request_id": "req-1", "step": "a"});
        let payload_b = json!({"request_id": "req-1", "step": "b"});
        assert_eq!(
            stable_sample_json_payload("json_logs", &payload_a, 0.5),
            stable_sample_json_payload("json_logs", &payload_b, 0.5)
        );
    }
}
