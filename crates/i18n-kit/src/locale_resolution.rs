use super::catalog::Catalog;
use super::locale::{
    Locale, locale_resolution_candidates, normalize_locale_request, normalize_system_locale_request,
};
use super::locale_error::ResolveLocaleError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocaleRequest<'a> {
    Explicit(&'a str),
    Environment(&'a str),
}

#[derive(Debug, Clone)]
struct CatalogLocaleSnapshot {
    default_locale: Locale,
    available_locales: Vec<Locale>,
}

impl CatalogLocaleSnapshot {
    fn capture<C>(catalog: &C) -> Self
    where
        C: Catalog + ?Sized,
    {
        Self {
            default_locale: catalog.default_locale(),
            available_locales: catalog.available_locales(),
        }
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.available_locales.contains(&locale)
    }

    fn resolve_request(
        &self,
        request: Option<LocaleRequest<'_>>,
    ) -> Result<Locale, ResolveLocaleError> {
        match request {
            Some(LocaleRequest::Explicit(requested)) => self.resolve_explicit(requested),
            Some(LocaleRequest::Environment(requested)) => self.resolve_environment(requested),
            None => self.resolve_default(),
        }
    }

    fn resolve_explicit(&self, requested: &str) -> Result<Locale, ResolveLocaleError> {
        let parts = normalize_locale_request(requested).ok_or_else(|| {
            ResolveLocaleError::UnknownLocale {
                requested: requested.to_string(),
            }
        })?;
        let candidates = locale_resolution_candidates(&parts);
        for locale in candidates {
            if self.locale_enabled(locale) {
                return Ok(locale);
            }
        }

        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: requested.to_string(),
            available: self.available_locales.clone(),
        })
    }

    fn resolve_environment(&self, requested: &str) -> Result<Locale, ResolveLocaleError> {
        let Some(parts) = normalize_system_locale_request(requested) else {
            return self.resolve_default();
        };

        let candidates = locale_resolution_candidates(&parts);
        for locale in candidates {
            if self.locale_enabled(locale) {
                return Ok(locale);
            }
        }

        self.resolve_default()
    }

    fn resolve_default(&self) -> Result<Locale, ResolveLocaleError> {
        if self.locale_enabled(self.default_locale) {
            return Ok(self.default_locale);
        }

        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: self.default_locale.to_string(),
            available: self.available_locales.clone(),
        })
    }
}

pub(crate) fn resolve_locale_request<C>(
    catalog: &C,
    request: Option<LocaleRequest<'_>>,
) -> Result<Locale, ResolveLocaleError>
where
    C: Catalog + ?Sized,
{
    CatalogLocaleSnapshot::capture(catalog).resolve_request(request)
}

#[cfg(test)]
pub(crate) fn resolve_environment_locale_request<C>(
    catalog: &C,
    requested: &str,
) -> Result<Locale, ResolveLocaleError>
where
    C: Catalog + ?Sized,
{
    CatalogLocaleSnapshot::capture(catalog).resolve_environment(requested)
}

#[cfg(test)]
pub(crate) fn select_locale_request<'a>(
    requested_locale: Option<&'a str>,
    env_locale: Option<&'a str>,
) -> Option<LocaleRequest<'a>> {
    requested_locale.map(LocaleRequest::Explicit).or_else(|| {
        env_locale
            .filter(|value| !super::locale::is_posix_default_locale_request(value))
            .map(LocaleRequest::Environment)
    })
}
