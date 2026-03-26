use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use super::catalog::Catalog;
use super::catalog_json::parse_json_catalog_text_map;
use super::catalog_state::{CatalogState, LocaleCatalogMap};
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

#[derive(Debug)]
pub struct StaticJsonCatalog {
    state: CatalogState,
}

impl StaticJsonCatalog {
    pub fn try_new(
        default_locale: Locale,
        locales: &[StaticJsonLocale],
    ) -> Result<Self, StaticCatalogError> {
        let parsed = parse_static_catalog(default_locale, locales)?;
        Ok(Self {
            state: CatalogState::new(default_locale, parsed),
        })
    }

    fn lookup_catalog_text(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.state.lookup(locale, key)
    }
}

impl TranslationCatalog for StaticJsonCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        if let Some(value) = self.lookup_catalog_text(locale, key) {
            return TranslationResolution::Exact(value);
        }

        if let Some(value) = self.state.lookup_default(locale, key) {
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
        self.state.default_locale()
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.state.available_locales()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.state.locale_enabled(locale)
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
) -> Result<LocaleCatalogMap, StaticCatalogError> {
    let mut parsed = LocaleCatalogMap::new();
    for source in locales.iter().filter(|source| source.enabled) {
        let texts = parse_json_catalog_text_map(source.json).map_err(|error| {
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
