use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::{Catalog, Locale, TranslationCatalog, TranslationResolution};

use super::{
    DynamicCatalogError,
    locale_sources::{
        LocaleMap, load_locales_from_directory, load_locales_from_json, load_locales_from_sources,
    },
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
    locales: RwLock<LocaleMap>,
    default_locale: Locale,
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
            locales: RwLock::new(LocaleMap::new()),
            default_locale,
            fallback_strategy,
        }
    }

    /// Creates an empty catalog.
    ///
    /// Prefer [`Self::empty`] when the unloaded state is intentional. This
    /// compatibility constructor exists so older call sites do not break.
    #[must_use]
    pub fn new(default_locale: Locale, fallback_strategy: FallbackStrategy) -> Self {
        Self::empty(default_locale, fallback_strategy)
    }

    fn with_locales(
        locales: LocaleMap,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Self {
        Self {
            locales: RwLock::new(locales),
            default_locale,
            fallback_strategy,
        }
    }

    pub fn from_directory(
        path: &Path,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Result<Self, DynamicCatalogError> {
        let locales = load_locales_from_directory(path, default_locale)?;
        Ok(Self::with_locales(
            locales,
            default_locale,
            fallback_strategy,
        ))
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

    pub fn reload_from_directory(&self, path: &Path) -> Result<(), DynamicCatalogError> {
        let locales = load_locales_from_directory(path, self.default_locale)?;
        *write_unpoisoned(&self.locales) = locales;
        Ok(())
    }

    /// Reloads the catalog from caller-provided locale sources.
    ///
    /// Source labels follow the same rules as [`Self::from_locale_sources`].
    pub fn reload_from_locale_sources<I, P>(&self, sources: I) -> Result<(), DynamicCatalogError>
    where
        I: IntoIterator<Item = (P, String)>,
        P: Into<PathBuf>,
    {
        let locales = load_locales_from_sources(sources, self.default_locale)?;
        *write_unpoisoned(&self.locales) = locales;
        Ok(())
    }

    #[must_use]
    pub fn fallback_strategy(&self) -> FallbackStrategy {
        self.fallback_strategy
    }

    fn lookup_catalog_text_in(locales: &LocaleMap, locale: Locale, key: &str) -> Option<Arc<str>> {
        locales
            .get(&locale)
            .and_then(|texts| texts.get(key))
            .cloned()
    }

    fn lookup_default_catalog_text_in(
        &self,
        locales: &LocaleMap,
        locale: Locale,
        key: &str,
    ) -> Option<Arc<str>> {
        (locale != self.default_locale)
            .then(|| Self::lookup_catalog_text_in(locales, self.default_locale, key))
            .flatten()
    }

    fn lookup_catalog_text(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        Self::lookup_catalog_text_in(&read_unpoisoned(&self.locales), locale, key)
    }
}

impl TranslationCatalog for DynamicJsonCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        let locales = read_unpoisoned(&self.locales);
        if let Some(value) = Self::lookup_catalog_text_in(&locales, locale, key) {
            return TranslationResolution::Exact(value);
        }

        match self.fallback_strategy {
            FallbackStrategy::ReturnKey => TranslationResolution::Synthetic(Arc::<str>::from(key)),
            FallbackStrategy::ReturnDefaultLocale => self
                .lookup_default_catalog_text_in(&locales, locale, key)
                .map_or(
                    TranslationResolution::Missing,
                    TranslationResolution::Fallback,
                ),
            FallbackStrategy::Both => {
                if let Some(value) = self.lookup_default_catalog_text_in(&locales, locale, key) {
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
        self.default_locale
    }

    fn available_locales(&self) -> Vec<Locale> {
        read_unpoisoned(&self.locales).keys().copied().collect()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        read_unpoisoned(&self.locales).contains_key(&locale)
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}
