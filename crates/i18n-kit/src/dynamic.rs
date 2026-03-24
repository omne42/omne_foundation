use std::collections::{BTreeMap, btree_map::Entry};
use std::fmt::{self, Display, Formatter};
use std::io;
use std::sync::Arc;

use serde::de::{self, Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

use crate::Locale;

mod catalog;
mod composed;
mod directory_loader;
mod locale_sources;
#[cfg(test)]
mod tests;

pub use self::catalog::{DynamicJsonCatalog, FallbackStrategy};
pub use self::composed::ComposedCatalog;

#[derive(Debug)]
pub enum DynamicCatalogError {
    Io(io::Error),
    Json(serde_json::Error),
    LocaleSourceJson {
        path: String,
        error: serde_json::Error,
    },
    InvalidLocaleIdentifier(String),
    InvalidLocaleFileName(String),
    DuplicateLocaleFile {
        locale: Locale,
        first_path: String,
        second_path: String,
    },
    TooManyLocaleSources {
        max: usize,
    },
    TooManyCatalogDirectories {
        max: usize,
    },
    CatalogDirectoryTooWide {
        path: String,
        entries: usize,
        max_entries: usize,
    },
    CatalogDirectoryTooDeep {
        path: String,
        depth: usize,
        max_depth: usize,
    },
    LocaleSourceTooLarge {
        path: String,
        bytes: usize,
        max_bytes: usize,
    },
    CatalogTooLarge {
        bytes: usize,
        max_bytes: usize,
    },
    MissingDefaultLocale(Locale),
}

impl Display for DynamicCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::LocaleSourceJson { path, error } => {
                write!(f, "invalid locale source JSON in {path}: {error}")
            }
            Self::InvalidLocaleIdentifier(locale) => {
                write!(f, "invalid locale identifier: {locale}")
            }
            Self::InvalidLocaleFileName(path) => {
                write!(f, "invalid locale file name: {path}")
            }
            Self::DuplicateLocaleFile {
                locale,
                first_path,
                second_path,
            } => write!(
                f,
                "duplicate locale file for {locale}: {first_path} and {second_path}"
            ),
            Self::TooManyLocaleSources { max } => {
                write!(f, "catalog exceeds locale source limit of {max} files")
            }
            Self::TooManyCatalogDirectories { max } => {
                write!(
                    f,
                    "catalog exceeds directory traversal limit of {max} directories"
                )
            }
            Self::CatalogDirectoryTooWide {
                path,
                entries,
                max_entries,
            } => write!(
                f,
                "catalog directory exceeds entry limit ({entries} > {max_entries}): {path}"
            ),
            Self::CatalogDirectoryTooDeep {
                path,
                depth,
                max_depth,
            } => write!(
                f,
                "catalog directory exceeds depth limit ({depth} > {max_depth}): {path}"
            ),
            Self::LocaleSourceTooLarge {
                path,
                bytes,
                max_bytes,
            } => write!(
                f,
                "catalog locale source exceeds size limit ({bytes} > {max_bytes} bytes): {path}"
            ),
            Self::CatalogTooLarge { bytes, max_bytes } => write!(
                f,
                "catalog exceeds total size limit ({bytes} > {max_bytes} bytes)"
            ),
            Self::MissingDefaultLocale(locale) => {
                write!(f, "default locale is missing from catalog: {locale}")
            }
        }
    }
}

impl std::error::Error for DynamicCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::LocaleSourceJson { error, .. } => Some(error),
            Self::InvalidLocaleIdentifier(_)
            | Self::InvalidLocaleFileName(_)
            | Self::DuplicateLocaleFile { .. }
            | Self::TooManyLocaleSources { .. }
            | Self::TooManyCatalogDirectories { .. }
            | Self::CatalogDirectoryTooWide { .. }
            | Self::CatalogDirectoryTooDeep { .. }
            | Self::LocaleSourceTooLarge { .. }
            | Self::CatalogTooLarge { .. }
            | Self::MissingDefaultLocale(_) => None,
        }
    }
}

impl From<io::Error> for DynamicCatalogError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DynamicCatalogError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub(crate) fn parse_json_catalog_text_map(
    json: &str,
) -> Result<BTreeMap<String, Arc<str>>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let texts = UniqueCatalogTextMap::deserialize(&mut deserializer)?.0;
    deserializer.end()?;
    Ok(texts)
}

fn parse_json_catalog_sources(
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
