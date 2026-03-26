use std::collections::BTreeMap;
use std::sync::Arc;

use crate::Locale;

pub(crate) type LocaleTexts = BTreeMap<String, Arc<str>>;
pub(crate) type LocaleCatalogMap = BTreeMap<Locale, LocaleTexts>;

#[derive(Debug)]
pub(crate) struct CatalogState {
    default_locale: Locale,
    locales: LocaleCatalogMap,
}

impl CatalogState {
    pub(crate) fn new(default_locale: Locale, locales: LocaleCatalogMap) -> Self {
        Self {
            default_locale,
            locales,
        }
    }

    pub(crate) fn default_locale(&self) -> Locale {
        self.default_locale
    }

    pub(crate) fn lookup(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.locales
            .get(&locale)
            .and_then(|texts| texts.get(key))
            .cloned()
    }

    pub(crate) fn lookup_default(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        (locale != self.default_locale)
            .then(|| self.lookup(self.default_locale, key))
            .flatten()
    }

    pub(crate) fn available_locales(&self) -> Vec<Locale> {
        self.locales.keys().copied().collect()
    }

    pub(crate) fn locale_enabled(&self, locale: Locale) -> bool {
        self.locales.contains_key(&locale)
    }
}
