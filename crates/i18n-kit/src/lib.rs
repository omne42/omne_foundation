mod catalog;
pub mod dynamic;
mod global_catalog;
mod locale;
mod locale_selection;
mod static_catalog;
#[cfg(test)]
mod tests;
mod translation;

pub use catalog::Catalog;
pub use dynamic::{ComposedCatalog, DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy};
pub use global_catalog::GlobalCatalog;
pub use locale::Locale;
pub use locale_selection::{
    ResolveLocaleError, resolve_locale_from_argv, resolve_locale_from_cli_args,
};
pub use static_catalog::{StaticCatalogError, StaticJsonCatalog, StaticJsonLocale};
pub use translation::{
    TemplateArg, TranslationCatalog, TranslationResolution, interpolate, render_structured_text,
};
