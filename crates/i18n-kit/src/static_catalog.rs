use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use super::catalog::Catalog;
use super::locale::Locale;
use super::translation::{TranslationCatalog, TranslationResolution};

#[derive(Debug, Clone, Copy)]
pub struct StaticJsonLocale {
    pub locale: Locale,
    pub enabled: bool,
    pub json: &'static str,
}

impl StaticJsonLocale {
    #[must_use]
    pub const fn new(locale: Locale, enabled: bool, json: &'static str) -> Self {
        Self {
            locale,
            enabled,
            json,
        }
    }
}

#[derive(Debug)]
pub enum StaticCatalogError {
    DuplicateEnabledLocale(Locale),
    MissingDefaultLocale(Locale),
    InvalidLocaleJson {
        locale: Locale,
        error: serde_json::Error,
    },
}

impl Display for StaticCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateEnabledLocale(locale) => {
                write!(f, "duplicate enabled locale in static catalog: {locale}")
            }
            Self::MissingDefaultLocale(locale) => {
                write!(
                    f,
                    "default locale must be enabled in static catalog: {locale}"
                )
            }
            Self::InvalidLocaleJson { locale, error } => {
                write!(f, "invalid static catalog JSON for {locale}: {error}")
            }
        }
    }
}

impl std::error::Error for StaticCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidLocaleJson { error, .. } => Some(error),
            Self::DuplicateEnabledLocale(_) | Self::MissingDefaultLocale(_) => None,
        }
    }
}

type ParsedCatalogMap = BTreeMap<Locale, BTreeMap<String, Arc<str>>>;

#[derive(Debug)]
pub struct StaticJsonCatalog {
    default_locale: Locale,
    parsed: ParsedCatalogMap,
}

impl StaticJsonCatalog {
    pub fn try_new(
        default_locale: Locale,
        locales: &[StaticJsonLocale],
    ) -> Result<Self, StaticCatalogError> {
        let parsed = parse_static_catalog(default_locale, locales)?;
        Ok(Self {
            default_locale,
            parsed,
        })
    }

    fn parsed_locales(&self) -> &ParsedCatalogMap {
        &self.parsed
    }

    fn lookup_catalog_text(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.parsed_locales()
            .get(&locale)
            .and_then(|texts| texts.get(key))
            .cloned()
    }
}

impl TranslationCatalog for StaticJsonCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        if let Some(value) = self.lookup_catalog_text(locale, key) {
            return TranslationResolution::Exact(value);
        }

        let fallback = self.default_locale;
        if fallback != locale
            && let Some(value) = self.lookup_catalog_text(fallback, key)
        {
            return TranslationResolution::Fallback(value);
        }

        TranslationResolution::Missing
    }
}

impl Catalog for StaticJsonCatalog {
    fn lookup_shared(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.lookup_catalog_text(locale, key)
    }

    fn default_locale(&self) -> Locale {
        self.default_locale
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.parsed_locales().keys().copied().collect()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.parsed_locales().contains_key(&locale)
    }
}

#[macro_export]
macro_rules! static_json_catalog {
    (
        default: $default_locale:expr,
        $($locale:expr => {
            enabled: $enabled:expr,
            json: $json:expr
        }),+ $(,)?
    ) => {{
        const SOURCES: &[$crate::StaticJsonLocale] = &[
            $(
                $crate::StaticJsonLocale::new($locale, $enabled, $json),
            )+
        ];
        $crate::StaticJsonCatalog::try_new($default_locale, SOURCES)
    }};
}

fn parse_static_catalog(
    default_locale: Locale,
    locales: &[StaticJsonLocale],
) -> Result<ParsedCatalogMap, StaticCatalogError> {
    let mut parsed = BTreeMap::new();
    for source in locales.iter().filter(|source| source.enabled) {
        let texts = crate::dynamic::parse_json_catalog_text_map(source.json).map_err(|error| {
            StaticCatalogError::InvalidLocaleJson {
                locale: source.locale,
                error,
            }
        })?;
        if parsed.insert(source.locale, texts).is_some() {
            return Err(StaticCatalogError::DuplicateEnabledLocale(source.locale));
        }
    }

    if parsed.contains_key(&default_locale) {
        return Ok(parsed);
    }

    Err(StaticCatalogError::MissingDefaultLocale(default_locale))
}
