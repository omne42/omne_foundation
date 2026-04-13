use std::borrow::Cow;
use std::fmt::{self, Display, Formatter};

use structured_text_kit::{
    CatalogArgValueRef, CatalogTextRef, StructuredText, StructuredTextRef,
    StructuredTextValidationError, try_structured_text,
};

use super::locale::Locale;
use super::translation::{TemplateArg, TranslationCatalog, interpolate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveLocaleError {
    UnknownLocale {
        requested: String,
    },
    LocaleNotEnabled {
        requested: String,
        available: Vec<Locale>,
    },
}

impl ResolveLocaleError {
    #[must_use]
    pub fn to_structured_text(&self) -> StructuredText {
        match self {
            Self::UnknownLocale { requested } => build_structured_text(
                try_structured_text!("locale.unknown", "requested" => requested.as_str()),
                self.to_string(),
            ),
            Self::LocaleNotEnabled {
                requested,
                available,
            } if available.is_empty() => build_structured_text(
                try_structured_text!(
                    "locale.not_enabled.none",
                    "requested" => requested.as_str()
                ),
                self.to_string(),
            ),
            Self::LocaleNotEnabled {
                requested,
                available,
            } => build_structured_text(
                try_structured_text!(
                    "locale.not_enabled.available",
                    "requested" => requested.as_str(),
                    "available" => format_locales(available)
                ),
                self.to_string(),
            ),
        }
    }

    #[must_use]
    pub fn render<C>(&self, catalog: &C, locale: Locale) -> String
    where
        C: TranslationCatalog + ?Sized,
    {
        render_locale_error_text(catalog, locale, &self.to_structured_text())
            .unwrap_or_else(|| self.to_string())
    }
}

impl Display for ResolveLocaleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownLocale { requested } => {
                write!(f, "unknown locale identifier: {requested}")
            }
            Self::LocaleNotEnabled {
                requested,
                available,
            } if available.is_empty() => {
                write!(
                    f,
                    "locale is not enabled: {requested}; no locales are available"
                )
            }
            Self::LocaleNotEnabled {
                requested,
                available,
            } => write!(
                f,
                "locale is not enabled: {requested}; available locales: {}",
                format_locales(available)
            ),
        }
    }
}

impl std::error::Error for ResolveLocaleError {}

fn build_structured_text(
    text: Result<StructuredText, StructuredTextValidationError>,
    fallback: impl Into<String>,
) -> StructuredText {
    text.unwrap_or_else(|_| StructuredText::freeform(fallback))
}

fn format_locales(locales: &[Locale]) -> String {
    let mut rendered = String::new();

    for (index, locale) in locales.iter().enumerate() {
        if index != 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(locale.as_str());
    }

    rendered
}

fn render_locale_error_text<C>(catalog: &C, locale: Locale, text: &StructuredText) -> Option<String>
where
    C: TranslationCatalog + ?Sized,
{
    render_locale_error_text_ref(catalog, locale, text.text_ref())
}

fn render_locale_error_text_ref<C>(
    catalog: &C,
    locale: Locale,
    text: StructuredTextRef<'_>,
) -> Option<String>
where
    C: TranslationCatalog + ?Sized,
{
    match text {
        StructuredTextRef::Catalog(text) => render_catalog_error_text(catalog, locale, text),
        StructuredTextRef::Freeform(text) => Some(text.to_owned()),
    }
}

fn render_catalog_error_text<C>(
    catalog: &C,
    locale: Locale,
    catalog_text: CatalogTextRef<'_>,
) -> Option<String>
where
    C: TranslationCatalog + ?Sized,
{
    let template = catalog.get_template_shared(locale, catalog_text.code())?;
    let raw_args = catalog_text.iter_args();
    if raw_args.len() == 0 {
        return Some(template.as_ref().to_owned());
    }

    let mut args = Vec::with_capacity(raw_args.len());
    for arg in raw_args {
        let value = match arg.value() {
            CatalogArgValueRef::Text(value) => Cow::Borrowed(value),
            CatalogArgValueRef::NestedText(text) => {
                Cow::Owned(render_locale_error_text_ref(catalog, locale, text)?)
            }
            CatalogArgValueRef::Bool(value) => Cow::Owned(value.to_string()),
            CatalogArgValueRef::Signed(value) => Cow::Owned(value.to_string()),
            CatalogArgValueRef::Unsigned(value) => Cow::Owned(value.to_string()),
            other => Cow::Owned(other.to_string()),
        };

        args.push(TemplateArg::new(arg.name(), value));
    }

    Some(interpolate(template.as_ref(), &args))
}

#[cfg(test)]
mod tests {
    use super::build_structured_text;
    use structured_text_kit::StructuredTextValidationError;

    #[test]
    fn build_structured_text_falls_back_to_freeform_on_invalid_code() {
        let text = build_structured_text(
            Err(StructuredTextValidationError::InvalidCode(
                "bad code".to_string(),
            )),
            "locale error fallback",
        );

        assert_eq!(text.freeform_text(), Some("locale error fallback"));
    }

    #[test]
    fn build_structured_text_falls_back_to_freeform_on_invalid_arg_name() {
        let text = build_structured_text(
            Err(StructuredTextValidationError::InvalidArgName(
                "bad arg".to_string(),
            )),
            "locale error arg fallback",
        );

        assert_eq!(text.freeform_text(), Some("locale error arg fallback"));
    }
}
