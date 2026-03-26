mod catalog;
mod catalog_json;
mod catalog_state;
mod dynamic;
mod locale;
mod locale_error;
mod locale_resolution;
mod static_catalog;
#[cfg(test)]
mod tests;
mod translation;

pub use catalog::Catalog;
pub use dynamic::{
    ComposedCatalog, DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy,
    MAX_CATALOG_TOTAL_BYTES, MAX_LOCALE_SOURCE_BYTES, MAX_LOCALE_SOURCES,
    validate_locale_source_limits, validate_locale_source_path,
};
pub use locale::Locale;
pub use locale_error::ResolveLocaleError;
pub use static_catalog::{StaticCatalogError, StaticJsonCatalog, StaticJsonLocale};
pub use translation::{
    TemplateArg, TranslationCatalog, TranslationResolution, interpolate, render_structured_text,
};
