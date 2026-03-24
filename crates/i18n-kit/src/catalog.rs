use std::sync::Arc;

use super::locale::Locale;
use super::locale_selection::{LocaleRequest, ResolveLocaleError, resolve_locale_request};
use super::translation::{TranslationCatalog, TranslationResolution};

pub trait Catalog: TranslationCatalog {
    /// Performs an exact locale lookup without applying any fallback.
    fn lookup_shared(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        match self.resolve_shared(locale, key) {
            TranslationResolution::Exact(value) => Some(value),
            TranslationResolution::Fallback(_)
            | TranslationResolution::Synthetic(_)
            | TranslationResolution::Missing => None,
        }
    }

    fn default_locale(&self) -> Locale;

    fn available_locales(&self) -> Vec<Locale>;

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.available_locales().contains(&locale)
    }

    fn try_resolve_locale(&self, requested: Option<&str>) -> Result<Locale, ResolveLocaleError> {
        let request = requested
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(LocaleRequest::Explicit);
        resolve_locale_request(self, request)
    }

    /// Resolves a locale request that came from environment-style syntax.
    fn try_resolve_environment_locale(
        &self,
        requested: &str,
    ) -> Result<Locale, ResolveLocaleError> {
        resolve_locale_request(self, Some(LocaleRequest::Environment(requested)))
    }
}
