use std::sync::{Arc, RwLock};

use super::catalog::Catalog;
use super::locale::Locale;
use super::locale_selection::ResolveLocaleError;
use super::translation::{TranslationCatalog, TranslationResolution};

/// Thread-safe runtime catalog handle.
///
/// Readers clone the current `Arc<dyn Catalog>` behind a shared `RwLock` and
/// run catalog code after the lock has been released. Replacements only
/// contend on swapping the pointer, so slow reads do not stall hot reloads.
///
/// Catalog-key lookups on an uninitialized handle behave like misses. APIs that
/// can report configuration problems explicitly keep returning
/// `ResolveLocaleError::CatalogNotInitialized`.
pub struct GlobalCatalog {
    default_locale: Locale,
    inner: RwLock<Option<Arc<dyn Catalog>>>,
}

impl GlobalCatalog {
    #[must_use]
    pub const fn new(default_locale: Locale) -> Self {
        Self {
            default_locale,
            inner: RwLock::new(None),
        }
    }

    pub fn replace<C>(&self, catalog: C)
    where
        C: Catalog + 'static,
    {
        *write_unpoisoned(&self.inner) = Some(Arc::new(catalog));
    }

    fn current_catalog(&self) -> Option<Arc<dyn Catalog>> {
        read_unpoisoned(&self.inner).as_ref().map(Arc::clone)
    }

    fn with_catalog<T>(&self, f: impl FnOnce(&dyn Catalog) -> T) -> Option<T> {
        self.current_catalog().map(|catalog| f(catalog.as_ref()))
    }
}

impl TranslationCatalog for GlobalCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        self.with_catalog(|catalog| catalog.resolve_shared(locale, key))
            .unwrap_or(TranslationResolution::Missing)
    }
}

impl Catalog for GlobalCatalog {
    fn try_resolve_locale(&self, requested: Option<&str>) -> Result<Locale, ResolveLocaleError> {
        self.with_catalog(|catalog| catalog.try_resolve_locale(requested))
            .unwrap_or(Err(ResolveLocaleError::CatalogNotInitialized))
    }

    fn try_resolve_environment_locale(
        &self,
        requested: &str,
    ) -> Result<Locale, ResolveLocaleError> {
        self.with_catalog(|catalog| catalog.try_resolve_environment_locale(requested))
            .unwrap_or(Err(ResolveLocaleError::CatalogNotInitialized))
    }

    fn default_locale(&self) -> Locale {
        self.with_catalog(|catalog| catalog.default_locale())
            .unwrap_or(self.default_locale)
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.with_catalog(|catalog| catalog.available_locales())
            .unwrap_or_default()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.with_catalog(|catalog| catalog.locale_enabled(locale))
            .unwrap_or(false)
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}
