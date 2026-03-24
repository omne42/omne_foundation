use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{Catalog, Locale, TranslationCatalog, TranslationResolution};

/// Composed catalog that checks exact matches across all catalogs first, then
/// respects per-catalog fallback order, and only then falls back to the
/// composed default locale.
pub struct ComposedCatalog {
    catalogs: Vec<Arc<dyn Catalog>>,
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
    pub fn add_catalog<C>(mut self, catalog: C) -> Self
    where
        C: Catalog + 'static,
    {
        self.catalogs.push(Arc::new(catalog));
        self
    }

    #[must_use]
    pub fn add_catalog_arc(mut self, catalog: Arc<dyn Catalog>) -> Self {
        self.catalogs.push(catalog);
        self
    }

    fn lookup_across_catalogs(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        for catalog in &self.catalogs {
            if let Some(value) = catalog.lookup_shared(locale, key) {
                return Some(value);
            }
        }
        None
    }

    fn lookup_default_locale_across_catalogs(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        let fallback = Catalog::default_locale(self);
        if locale != fallback {
            return self.lookup_across_catalogs(fallback, key);
        }

        None
    }
}

impl TranslationCatalog for ComposedCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        let mut fallback = None;
        let mut synthetic = None;
        for catalog in &self.catalogs {
            match catalog.resolve_shared(locale, key) {
                TranslationResolution::Exact(value) => return TranslationResolution::Exact(value),
                TranslationResolution::Fallback(value) if fallback.is_none() => {
                    fallback = Some(value);
                }
                TranslationResolution::Fallback(_) => {}
                TranslationResolution::Synthetic(value) if synthetic.is_none() => {
                    synthetic = Some(value);
                }
                TranslationResolution::Synthetic(_) | TranslationResolution::Missing => {}
            }
        }

        if let Some(value) = fallback {
            return TranslationResolution::Fallback(value);
        }

        if let Some(value) = self.lookup_default_locale_across_catalogs(locale, key) {
            return TranslationResolution::Fallback(value);
        }

        synthetic.map_or(
            TranslationResolution::Missing,
            TranslationResolution::Synthetic,
        )
    }
}

impl Catalog for ComposedCatalog {
    fn lookup_shared(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.lookup_across_catalogs(locale, key)
    }

    fn default_locale(&self) -> Locale {
        self.default_locale
    }

    fn available_locales(&self) -> Vec<Locale> {
        let mut locales = BTreeSet::new();
        for catalog in &self.catalogs {
            locales.extend(catalog.available_locales());
        }
        locales.into_iter().collect()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.catalogs
            .iter()
            .any(|catalog| catalog.locale_enabled(locale))
    }
}
