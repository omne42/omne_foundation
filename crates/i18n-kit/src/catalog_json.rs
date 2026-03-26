use std::collections::{BTreeMap, btree_map::Entry};
use std::fmt::{self, Formatter};
use std::sync::Arc;

use serde::de::{self, Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

pub(crate) fn parse_json_catalog_text_map(
    json: &str,
) -> Result<BTreeMap<String, Arc<str>>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let texts = UniqueCatalogTextMap::deserialize(&mut deserializer)?.0;
    deserializer.end()?;
    Ok(texts)
}

pub(crate) fn parse_json_catalog_sources(
    json: &str,
) -> Result<BTreeMap<String, Box<RawValue>>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let catalog = UniqueRawLocaleMap::deserialize(&mut deserializer)?.0;
    deserializer.end()?;
    Ok(catalog)
}

fn validate_catalog_key(key: &str) -> Result<(), String> {
    if is_valid_catalog_identifier(key) {
        return Ok(());
    }

    Err(format!("invalid catalog key: {key}"))
}

fn validate_catalog_template(key: &str, template: &str) -> Result<(), String> {
    let mut offset = 0usize;

    while offset < template.len() {
        let rest = &template[offset..];
        let Some(marker_offset) = rest.find(['{', '}']) else {
            return Ok(());
        };

        let marker_index = offset + marker_offset;
        match template.as_bytes()[marker_index] {
            b'{' => {
                let placeholder_tail = &template[marker_index + 1..];
                let Some(end_offset) = placeholder_tail.find('}') else {
                    return Err(format!(
                        "invalid catalog template for {key}: unclosed placeholder"
                    ));
                };
                let placeholder = &placeholder_tail[..end_offset];
                if placeholder.is_empty() {
                    return Err(format!(
                        "invalid catalog template for {key}: empty placeholder"
                    ));
                }
                if !is_valid_catalog_identifier(placeholder) {
                    return Err(format!(
                        "invalid catalog template for {key}: invalid placeholder name: {placeholder}"
                    ));
                }
                offset = marker_index + end_offset + 2;
            }
            b'}' => {
                return Err(format!(
                    "invalid catalog template for {key}: unmatched closing brace"
                ));
            }
            _ => unreachable!("find(['{{', '}}']) only returns brace bytes"),
        }
    }

    Ok(())
}

fn is_valid_catalog_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

struct UniqueCatalogTextMap(BTreeMap<String, Arc<str>>);

impl<'de> Deserialize<'de> for UniqueCatalogTextMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(UniqueCatalogTextMapVisitor)
    }
}

struct UniqueCatalogTextMapVisitor;

impl<'de> Visitor<'de> for UniqueCatalogTextMapVisitor {
    type Value = UniqueCatalogTextMap;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object mapping catalog keys to strings")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut texts = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            validate_catalog_key(&key).map_err(de::Error::custom)?;
            let value = map.next_value::<String>()?;
            validate_catalog_template(&key, &value).map_err(de::Error::custom)?;
            let value = Arc::<str>::from(value);
            match texts.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                Entry::Occupied(entry) => {
                    return Err(de::Error::custom(format!(
                        "duplicate catalog key: {}",
                        entry.key()
                    )));
                }
            }
        }
        Ok(UniqueCatalogTextMap(texts))
    }
}

struct UniqueRawLocaleMap(BTreeMap<String, Box<RawValue>>);

impl<'de> Deserialize<'de> for UniqueRawLocaleMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(UniqueRawLocaleMapVisitor)
    }
}

struct UniqueRawLocaleMapVisitor;

impl<'de> Visitor<'de> for UniqueRawLocaleMapVisitor {
    type Value = UniqueRawLocaleMap;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object mapping locale identifiers to catalog text maps")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut locales = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value::<Box<RawValue>>()?;
            match locales.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                Entry::Occupied(entry) => {
                    return Err(de::Error::custom(format!(
                        "duplicate locale identifier in JSON: {}",
                        entry.key()
                    )));
                }
            }
        }
        Ok(UniqueRawLocaleMap(locales))
    }
}
