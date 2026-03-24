use std::borrow::Cow;
use std::sync::Arc;

use structured_text_kit::{CatalogArgValueRef, CatalogTextRef, StructuredText, StructuredTextRef};

use super::locale::Locale;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateArg<'a> {
    name: Cow<'a, str>,
    value: Cow<'a, str>,
}

impl<'a> TemplateArg<'a> {
    #[must_use]
    pub fn new(name: impl Into<Cow<'a, str>>, value: impl Into<Cow<'a, str>>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    #[must_use]
    pub fn value(&self) -> &str {
        self.value.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslationResolution {
    /// Exact translation template for the requested locale.
    Exact(Arc<str>),
    /// Translation template found through locale fallback.
    Fallback(Arc<str>),
    /// Synthetic user-facing text such as a returned catalog key.
    ///
    /// This variant is already final text and must not be interpolated again.
    Synthetic(Arc<str>),
    /// No template or synthetic fallback is available.
    Missing,
}

impl TranslationResolution {
    #[must_use]
    pub fn into_text_shared(self) -> Option<Arc<str>> {
        match self {
            Self::Exact(value) | Self::Fallback(value) | Self::Synthetic(value) => Some(value),
            Self::Missing => None,
        }
    }

    #[must_use]
    pub fn into_template_shared(self) -> Option<Arc<str>> {
        match self {
            Self::Exact(value) | Self::Fallback(value) => Some(value),
            Self::Synthetic(_) | Self::Missing => None,
        }
    }
}

pub trait TranslationCatalog: Send + Sync {
    /// Resolves a catalog key while preserving whether it was an exact hit, a real
    /// fallback, a synthetic fallback, or a full miss.
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution;

    /// Returns user-facing text for the key, including synthetic fallbacks.
    fn get_text_shared(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.resolve_shared(locale, key).into_text_shared()
    }

    /// Returns a real translation template without synthesizing the key on a miss.
    fn get_template_shared(&self, locale: Locale, key: &str) -> Option<Arc<str>> {
        self.resolve_shared(locale, key).into_template_shared()
    }

    fn get_text(&self, locale: Locale, key: &str) -> Option<String> {
        self.get_text_shared(locale, key)
            .map(|value| value.as_ref().to_owned())
    }

    /// Renders a resolved catalog text.
    ///
    /// Exact and fallback templates are interpolated with `args`. Synthetic
    /// fallbacks and hard misses are treated as final text and are returned
    /// without reinterpreting placeholders from `key`.
    fn render_text(&self, locale: Locale, key: &str, args: &[TemplateArg<'_>]) -> String {
        match self.resolve_shared(locale, key) {
            TranslationResolution::Exact(template) | TranslationResolution::Fallback(template) => {
                interpolate(template.as_ref(), args)
            }
            TranslationResolution::Synthetic(text) => text.as_ref().to_owned(),
            TranslationResolution::Missing => key.to_owned(),
        }
    }
}

#[must_use]
pub fn render_structured_text<C>(catalog: &C, locale: Locale, text: &StructuredText) -> String
where
    C: TranslationCatalog + ?Sized,
{
    render_structured_text_ref(catalog, locale, text.text_ref())
}

fn render_structured_text_ref<C>(catalog: &C, locale: Locale, text: StructuredTextRef<'_>) -> String
where
    C: TranslationCatalog + ?Sized,
{
    match text {
        StructuredTextRef::Catalog(catalog_text) => {
            render_catalog_text(catalog, locale, catalog_text)
        }
        StructuredTextRef::Freeform(text) => text.to_string(),
    }
}

fn render_catalog_text<C>(catalog: &C, locale: Locale, catalog_text: CatalogTextRef<'_>) -> String
where
    C: TranslationCatalog + ?Sized,
{
    let raw_args = catalog_text.iter_args();
    if raw_args.len() == 0 {
        return catalog.render_text(locale, catalog_text.code(), &[]);
    }

    let mut args = Vec::with_capacity(raw_args.len());
    for arg in raw_args {
        let value = match arg.value() {
            CatalogArgValueRef::Text(value) => Cow::Borrowed(value),
            CatalogArgValueRef::NestedText(text) => {
                Cow::Owned(render_structured_text_ref(catalog, locale, text))
            }
            CatalogArgValueRef::Bool(value) => Cow::Owned(value.to_string()),
            CatalogArgValueRef::Signed(value) => Cow::Owned(value.to_string()),
            CatalogArgValueRef::Unsigned(value) => Cow::Owned(value.to_string()),
            other => Cow::Owned(other.to_string()),
        };

        args.push(TemplateArg::new(arg.name(), value));
    }

    catalog.render_text(locale, catalog_text.code(), &args)
}

#[must_use]
/// Performs a single-pass `{name}` substitution.
///
/// This is intentionally a small template helper, not a full translation-format engine:
/// no escaping, no plural/select grammar, and no recursive interpolation.
pub fn interpolate(template: &str, args: &[TemplateArg<'_>]) -> String {
    if args.is_empty() {
        return template.to_owned();
    }

    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        rendered.push_str(&rest[..start]);
        let Some(end) = rest[start + 1..].find('}') else {
            rendered.push_str(&rest[start..]);
            return rendered;
        };

        let name_start = start + 1;
        let name_end = name_start + end;
        let placeholder = &rest[name_start..name_end];
        if let Some(arg) = args.iter().rev().find(|arg| arg.name() == placeholder) {
            rendered.push_str(arg.value());
        } else {
            rendered.push('{');
            rendered.push_str(placeholder);
            rendered.push('}');
        }

        rest = &rest[name_end + 1..];
    }
    rendered.push_str(rest);
    rendered
}
