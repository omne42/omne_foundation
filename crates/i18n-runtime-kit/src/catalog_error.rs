use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::sync::Arc;

use i18n_kit::{
    DynamicCatalogError, Locale, ResolveLocaleError, TemplateArg, TranslationCatalog, interpolate,
};
use structured_text_kit::{
    CatalogArgValueRef, CatalogTextRef, StructuredText, StructuredTextRef, try_structured_text,
};

#[derive(Debug, Clone)]
pub struct CatalogInitError(Arc<dyn StdError + Send + Sync>);

impl CatalogInitError {
    pub fn new(error: impl StdError + Send + Sync + 'static) -> Self {
        Self(Arc::new(error))
    }
}

impl Display for CatalogInitError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.0.as_ref(), f)
    }
}

impl StdError for CatalogInitError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.0.as_ref())
    }
}

impl From<io::Error> for CatalogInitError {
    fn from(error: io::Error) -> Self {
        Self::new(error)
    }
}

impl From<DynamicCatalogError> for CatalogInitError {
    fn from(error: DynamicCatalogError) -> Self {
        Self::new(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliLocaleError {
    Resolve(ResolveLocaleError),
    MissingValue { flag: &'static str },
    DuplicateFlag { flag: &'static str },
    MisplacedFlag { flag: &'static str },
}

impl CliLocaleError {
    #[must_use]
    pub fn to_structured_text(&self) -> StructuredText {
        match self {
            Self::Resolve(error) => error.to_structured_text(),
            Self::MissingValue { flag } => {
                build_structured_text(try_structured_text!("cli.missing_value", "flag" => *flag))
            }
            Self::DuplicateFlag { flag } => {
                build_structured_text(try_structured_text!("cli.duplicate_flag", "flag" => *flag))
            }
            Self::MisplacedFlag { flag } => {
                build_structured_text(try_structured_text!("cli.misplaced_flag", "flag" => *flag))
            }
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

impl Display for CliLocaleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve(error) => Display::fmt(error, f),
            Self::MissingValue { flag } => write!(f, "missing value for locale flag {flag}"),
            Self::DuplicateFlag { flag } => {
                write!(f, "locale flag specified multiple times: {flag}")
            }
            Self::MisplacedFlag { flag } => {
                write!(
                    f,
                    "locale flag must appear before positional arguments: {flag}"
                )
            }
        }
    }
}

impl StdError for CliLocaleError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Resolve(error) => Some(error),
            Self::MissingValue { .. } | Self::DuplicateFlag { .. } | Self::MisplacedFlag { .. } => {
                None
            }
        }
    }
}

impl From<ResolveLocaleError> for CliLocaleError {
    fn from(error: ResolveLocaleError) -> Self {
        Self::Resolve(error)
    }
}

#[derive(Debug, Clone)]
pub enum CatalogLocaleError {
    Initialization(CatalogInitError),
    Resolve(ResolveLocaleError),
    Cli(CliLocaleError),
}

impl CatalogLocaleError {
    #[must_use]
    pub fn render<C>(&self, catalog: &C, locale: Locale) -> String
    where
        C: TranslationCatalog + ?Sized,
    {
        match self {
            Self::Initialization(error) => error.to_string(),
            Self::Resolve(error) => error.render(catalog, locale),
            Self::Cli(error) => error.render(catalog, locale),
        }
    }
}

impl Display for CatalogLocaleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialization(error) => Display::fmt(error, f),
            Self::Resolve(error) => Display::fmt(error, f),
            Self::Cli(error) => Display::fmt(error, f),
        }
    }
}

impl StdError for CatalogLocaleError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Initialization(error) => Some(error),
            Self::Resolve(error) => Some(error),
            Self::Cli(error) => Some(error),
        }
    }
}

fn build_structured_text(
    text: Result<StructuredText, structured_text_kit::StructuredTextValidationError>,
) -> StructuredText {
    text.expect("locale error structured-text schema must remain valid")
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
