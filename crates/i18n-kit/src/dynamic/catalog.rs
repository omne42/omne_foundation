use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::catalog_state::CatalogState;
use crate::{Catalog, Locale, TranslationCatalog, TranslationResolution};

use super::{
    DynamicCatalogError,
    locale_sources::{LocaleMap, load_locales_from_json, load_locales_from_sources},
};

/// Strategy for handling missing catalog keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackStrategy {
    /// Return the key itself when catalog text is missing.
    ReturnKey,
    /// Fall back to default locale when catalog text is missing.
    ReturnDefaultLocale,
    /// Try default locale first, then return key if still missing.
    Both,
}

/// Dynamic JSON catalog loaded from files or environment at runtime.
#[derive(Debug)]
pub struct DynamicJsonCatalog {
    state: RwLock<CatalogState>,
    fallback_strategy: FallbackStrategy,
}

impl DynamicJsonCatalog {
    /// Creates an explicitly empty catalog.
    ///
    /// This is only for staged loading, tests, or hot-reload setups that
    /// intentionally start without any locale data. If you already have data,
    /// use one of the `from_*` constructors instead.
    #[must_use]
    pub fn empty(default_locale: Locale, fallback_strategy: FallbackStrategy) -> Self {
        Self {
            state: RwLock::new(CatalogState::new(default_locale, LocaleMap::new())),
            fallback_strategy,
        }
    }

    fn with_locales(
        locales: LocaleMap,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Self {
        Self {
            state: RwLock::new(CatalogState::new(default_locale, locales)),
            fallback_strategy,
        }
    }

    pub fn from_json_string(
        json: &str,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Result<Self, DynamicCatalogError> {
        let locales = load_locales_from_json(json, default_locale)?;
        Ok(Self::with_locales(
            locales,
            default_locale,
            fallback_strategy,
        ))
    }

    /// Loads a catalog from caller-provided locale sources.
    ///
    /// Source labels may be either `en_US` or `en_US.json`. Any other
    /// extension is rejected instead of being silently ignored.
    pub fn from_locale_sources<I, P>(
        sources: I,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Result<Self, DynamicCatalogError>
    where
        I: IntoIterator<Item = (P, String)>,
        P: Into<PathBuf>,
    {
        let locales = load_locales_from_sources(sources, default_locale)?;
        Ok(Self::with_locales(
            locales,
            default_locale,
            fallback_strategy,
        ))
    }

    /// Reloads the catalog from caller-provided locale sources.
    ///
    /// Source labels follow the same rules as [`Self::from_locale_sources`].
    pub fn reload_from_locale_sources<I, P>(&self, sources: I) -> Result<(), DynamicCatalogError>
    where
        I: IntoIterator<Item = (P, String)>,
        P: Into<PathBuf>,
    {
        let default_locale = self.default_locale();
        let locales = load_locales_from_sources(sources, default_locale)?;
        *write_unpoisoned(&self.state) = CatalogState::new(default_locale, locales);
        Ok(())
    }

    #[must_use]
    pub fn fallback_strategy(&self) -> FallbackStrategy {
        self.fallback_strategy
    }

    fn lookup_catalog_text(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        read_unpoisoned(&self.state).lookup(locale, key)
    }
}

impl TranslationCatalog for DynamicJsonCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        let state = read_unpoisoned(&self.state);
        if let Some(value) = state.lookup(locale, key) {
            return TranslationResolution::Exact(value);
        }

        match self.fallback_strategy {
            FallbackStrategy::ReturnKey => TranslationResolution::Synthetic(Arc::<str>::from(key)),
            FallbackStrategy::ReturnDefaultLocale => state.lookup_default(locale, key).map_or(
                TranslationResolution::Missing,
                TranslationResolution::Fallback,
            ),
            FallbackStrategy::Both => {
                if let Some(value) = state.lookup_default(locale, key) {
                    TranslationResolution::Fallback(value)
                } else {
                    TranslationResolution::Synthetic(Arc::<str>::from(key))
                }
            }
        }
    }
}

impl Catalog for DynamicJsonCatalog {
    fn lookup_shared(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.lookup_catalog_text(locale, key)
    }

    fn default_locale(&self) -> Locale {
        read_unpoisoned(&self.state).default_locale()
    }

    fn available_locales(&self) -> Vec<Locale> {
        read_unpoisoned(&self.state).available_locales()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        read_unpoisoned(&self.state).locale_enabled(locale)
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}
