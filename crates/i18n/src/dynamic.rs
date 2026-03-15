use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::{Catalog, Locale, MessageCatalog};

#[derive(Debug)]
pub enum DynamicCatalogError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidLocaleFileName(String),
}

impl Display for DynamicCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::InvalidLocaleFileName(path) => {
                write!(f, "invalid locale file name: {path}")
            }
        }
    }
}

impl std::error::Error for DynamicCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidLocaleFileName(_) => None,
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

/// Strategy for handling missing translation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackStrategy {
    /// Return the key itself when translation is missing.
    ReturnKey,
    /// Fall back to default locale when translation is missing.
    ReturnDefaultLocale,
    /// Try default locale first, then return key if still missing.
    Both,
}

/// Dynamic JSON catalog loaded from files or environment at runtime.
#[derive(Debug)]
pub struct DynamicJsonCatalog {
    locales: Arc<RwLock<BTreeMap<Locale, BTreeMap<String, String>>>>,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
}

impl DynamicJsonCatalog {
    #[must_use]
    pub fn new(default_locale: Locale, fallback_strategy: FallbackStrategy) -> Self {
        Self {
            locales: Arc::new(RwLock::new(BTreeMap::new())),
            default_locale,
            fallback_strategy,
        }
    }

    pub fn from_directory(
        path: &Path,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Result<Self, DynamicCatalogError> {
        let catalog = Self::new(default_locale, fallback_strategy);
        *write_unpoisoned(&catalog.locales) = load_directory(path)?;
        Ok(catalog)
    }

    pub fn from_json_string(
        json: &str,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Result<Self, DynamicCatalogError> {
        let catalog = Self::new(default_locale, fallback_strategy);

        let data: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(json)?;
        let mut locales = BTreeMap::new();
        for (locale_str, messages) in data {
            if let Some(locale) = Locale::parse(&locale_str) {
                locales.insert(locale, messages);
            }
        }

        *write_unpoisoned(&catalog.locales) = locales;
        Ok(catalog)
    }

    pub fn reload_from_directory(&self, path: &Path) -> Result<(), DynamicCatalogError> {
        *write_unpoisoned(&self.locales) = load_directory(path)?;
        Ok(())
    }

    #[must_use]
    pub fn fallback_strategy(&self) -> FallbackStrategy {
        self.fallback_strategy
    }
}

impl MessageCatalog for DynamicJsonCatalog {
    fn get(&self, locale: Locale, key: &str) -> Option<String> {
        let locales = read_unpoisoned(&self.locales);
        let result = locales
            .get(&locale)
            .and_then(|messages| messages.get(key))
            .cloned();
        if result.is_some() {
            return result;
        }

        match self.fallback_strategy {
            FallbackStrategy::ReturnKey => Some(key.to_string()),
            FallbackStrategy::ReturnDefaultLocale => {
                if locale != self.default_locale {
                    locales
                        .get(&self.default_locale)
                        .and_then(|messages| messages.get(key))
                        .cloned()
                } else {
                    Some(key.to_string())
                }
            }
            FallbackStrategy::Both => {
                if locale != self.default_locale {
                    locales
                        .get(&self.default_locale)
                        .and_then(|messages| messages.get(key))
                        .cloned()
                        .or_else(|| Some(key.to_string()))
                } else {
                    Some(key.to_string())
                }
            }
        }
    }
}

impl Catalog for DynamicJsonCatalog {
    fn default_locale(&self) -> Locale {
        self.default_locale
    }

    fn available_locales(&self) -> Vec<Locale> {
        read_unpoisoned(&self.locales).keys().copied().collect()
    }
}

/// Composed catalog that tries multiple sources with fallback.
pub struct ComposedCatalog {
    catalogs: Vec<Arc<dyn MessageCatalog>>,
    default_locale: Locale,
}

impl ComposedCatalog {
    #[must_use]
    pub fn new(default_locale: Locale) -> Self {
        Self {
            catalogs: Vec::new(),
            default_locale,
        }
    }

    #[must_use]
    pub fn add_catalog(mut self, catalog: Arc<dyn MessageCatalog>) -> Self {
        self.catalogs.push(catalog);
        self
    }
}

impl MessageCatalog for ComposedCatalog {
    fn get(&self, locale: Locale, key: &str) -> Option<String> {
        for catalog in &self.catalogs {
            if let Some(value) = catalog.get(locale, key).filter(|value| value != key) {
                return Some(value);
            }
        }

        if locale != self.default_locale {
            for catalog in &self.catalogs {
                if let Some(value) = catalog
                    .get(self.default_locale, key)
                    .filter(|value| value != key)
                {
                    return Some(value);
                }
            }
        }

        Some(key.to_string())
    }
}

impl Catalog for ComposedCatalog {
    fn default_locale(&self) -> Locale {
        self.default_locale
    }

    fn available_locales(&self) -> Vec<Locale> {
        vec![self.default_locale]
    }
}

fn load_directory(
    path: &Path,
) -> Result<BTreeMap<Locale, BTreeMap<String, String>>, DynamicCatalogError> {
    let mut locales = BTreeMap::new();
    if !path.exists() {
        return Ok(locales);
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let file_name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                DynamicCatalogError::InvalidLocaleFileName(path.display().to_string())
            })?;
        let Some(locale) = Locale::parse(file_name) else {
            continue;
        };

        let content = fs::read_to_string(&path)?;
        let messages: BTreeMap<String, String> = serde_json::from_str(&content)?;
        locales.insert(locale, messages);
    }

    Ok(locales)
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().expect("DynamicJsonCatalog read lock poisoned")
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .expect("DynamicJsonCatalog write lock poisoned")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_catalog_from_json_string() {
        let json = r#"
        {
            "en_US": { "greeting": "hello {name}" },
            "fr_FR": { "greeting": "bonjour {name}" }
        }
        "#;

        let catalog =
            DynamicJsonCatalog::from_json_string(json, Locale::EnUs, FallbackStrategy::Both)
                .expect("load catalog");

        assert_eq!(
            catalog.get(Locale::EnUs, "greeting"),
            Some("hello {name}".to_string())
        );
        assert_eq!(
            catalog.get(Locale::parse("fr_FR").expect("fr"), "greeting"),
            Some("bonjour {name}".to_string())
        );
    }

    #[test]
    fn dynamic_catalog_fallback_strategy_return_default_locale() {
        let json = r#"
        {
            "en_US": { "greeting": "hello" }
        }
        "#;

        let catalog = DynamicJsonCatalog::from_json_string(
            json,
            Locale::EnUs,
            FallbackStrategy::ReturnDefaultLocale,
        )
        .expect("load catalog");

        assert_eq!(
            catalog.get(Locale::parse("fr_FR").expect("fr"), "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(
            catalog.get(Locale::parse("fr_FR").expect("fr"), "missing"),
            None
        );
    }

    #[test]
    fn composed_catalog_tries_multiple_sources() {
        let json1 = r#"{ "en_US": { "greeting": "hello" } }"#;
        let json2 = r#"{ "en_US": { "farewell": "goodbye" } }"#;

        let catalog1 =
            DynamicJsonCatalog::from_json_string(json1, Locale::EnUs, FallbackStrategy::ReturnKey)
                .expect("catalog1");
        let catalog2 =
            DynamicJsonCatalog::from_json_string(json2, Locale::EnUs, FallbackStrategy::ReturnKey)
                .expect("catalog2");

        let composed = ComposedCatalog::new(Locale::EnUs)
            .add_catalog(Arc::new(catalog1))
            .add_catalog(Arc::new(catalog2));

        assert_eq!(
            composed.get(Locale::EnUs, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(
            composed.get(Locale::EnUs, "farewell"),
            Some("goodbye".to_string())
        );
    }
}
