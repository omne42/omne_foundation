use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use ::i18n::{
    Catalog, DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy, GlobalCatalog, Locale,
    MessageCatalog,
};

use crate::{ResourceManifest, bootstrap_text_resources};

#[derive(Debug)]
pub enum ResourceCatalogError {
    Bootstrap(io::Error),
    Load(DynamicCatalogError),
    Reload(DynamicCatalogError),
}

impl Display for ResourceCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap(error) => write!(f, "bootstrap text resources: {error}"),
            Self::Load(error) => write!(f, "load i18n catalog: {error}"),
            Self::Reload(error) => write!(f, "reload i18n catalog: {error}"),
        }
    }
}

impl std::error::Error for ResourceCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bootstrap(error) => Some(error),
            Self::Load(error) => Some(error),
            Self::Reload(error) => Some(error),
        }
    }
}

pub struct ResourceBackedCatalog {
    root: PathBuf,
    inner: DynamicJsonCatalog,
}

impl ResourceBackedCatalog {
    pub fn bootstrap(
        root: impl Into<PathBuf>,
        manifest: &ResourceManifest,
        default_locale: Locale,
        fallback_strategy: FallbackStrategy,
    ) -> Result<Self, ResourceCatalogError> {
        let root = root.into();
        bootstrap_text_resources(&root, manifest).map_err(ResourceCatalogError::Bootstrap)?;
        let inner = DynamicJsonCatalog::from_directory(&root, default_locale, fallback_strategy)
            .map_err(ResourceCatalogError::Load)?;
        Ok(Self { root, inner })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn reload(&self) -> Result<(), ResourceCatalogError> {
        self.inner
            .reload_from_directory(&self.root)
            .map_err(ResourceCatalogError::Reload)
    }
}

impl MessageCatalog for ResourceBackedCatalog {
    fn get(&self, locale: Locale, key: &str) -> Option<String> {
        self.inner.get(locale, key)
    }
}

impl Catalog for ResourceBackedCatalog {
    fn default_locale(&self) -> Locale {
        self.inner.default_locale()
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.inner.available_locales()
    }
}

/// Lazily initializes a shared catalog exactly once per process.
///
/// Initialization is guarded by `OnceLock`, while steady-state reads and
/// replacements delegate to `GlobalCatalog`, which uses `RwLock` plus `Arc`.
pub struct LazyCatalog {
    inner: GlobalCatalog,
    initialized: OnceLock<Result<(), String>>,
    initializer: fn() -> Result<Arc<dyn Catalog>, String>,
}

impl LazyCatalog {
    pub const fn new(
        default_locale: Locale,
        initializer: fn() -> Result<Arc<dyn Catalog>, String>,
    ) -> Self {
        Self {
            inner: GlobalCatalog::new(default_locale),
            initialized: OnceLock::new(),
            initializer,
        }
    }

    #[must_use]
    pub fn default_locale(&self) -> Locale {
        self.ensure_initialized();
        self.inner.default_locale()
    }

    #[must_use]
    pub fn available_locales(&self) -> Vec<Locale> {
        self.ensure_initialized();
        self.inner.available_locales()
    }

    #[must_use]
    pub fn locale_enabled(&self, locale: Locale) -> bool {
        self.ensure_initialized();
        self.inner.locale_enabled(locale)
    }

    pub fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, String> {
        self.ensure_initialized();
        self.inner.resolve_locale(requested)
    }

    pub fn resolve_cli_locale(
        &self,
        args: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), String> {
        self.ensure_initialized();
        self.inner.resolve_cli_locale(args, env_var)
    }

    pub fn replace<C>(&self, catalog: C)
    where
        C: Catalog + 'static,
    {
        self.inner.replace(catalog);
        self.mark_initialized();
    }

    pub fn replace_arc(&self, catalog: Arc<dyn Catalog>) {
        self.inner.replace_arc(catalog);
        self.mark_initialized();
    }

    fn ensure_initialized(&self) {
        let _ = self.initialized.get_or_init(|| {
            let catalog = (self.initializer)()?;
            self.inner.replace_arc(catalog);
            Ok(())
        });
    }

    fn mark_initialized(&self) {
        let _ = self.initialized.set(Ok(()));
    }
}

impl MessageCatalog for LazyCatalog {
    fn get(&self, locale: Locale, key: &str) -> Option<String> {
        self.ensure_initialized();
        self.inner.get(locale, key)
    }
}

impl Catalog for LazyCatalog {
    fn default_locale(&self) -> Locale {
        self.ensure_initialized();
        self.inner.default_locale()
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.ensure_initialized();
        self.inner.available_locales()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[derive(Debug)]
    struct TestCatalog;

    impl MessageCatalog for TestCatalog {
        fn get(&self, locale: Locale, key: &str) -> Option<String> {
            match (locale.as_str(), key) {
                ("en_US", "greeting") => Some("hello".to_string()),
                ("fr_FR", "greeting") => Some("bonjour".to_string()),
                _ => None,
            }
        }
    }

    impl Catalog for TestCatalog {
        fn default_locale(&self) -> Locale {
            Locale::EnUs
        }

        fn available_locales(&self) -> Vec<Locale> {
            vec![Locale::EnUs, Locale::parse("fr_FR").expect("fr_FR")]
        }
    }

    fn failing_initializer() -> Result<Arc<dyn Catalog>, String> {
        Err("init failed".to_string())
    }

    #[test]
    fn lazy_catalog_replace_overrides_failed_initializer_state() {
        let catalog = LazyCatalog::new(Locale::EnUs, failing_initializer);

        assert_eq!(catalog.get(Locale::EnUs, "greeting"), None);

        catalog.replace(TestCatalog);

        assert_eq!(
            catalog.get(Locale::parse("fr_FR").expect("fr_FR"), "greeting"),
            Some("bonjour".to_string())
        );
        assert_eq!(catalog.default_locale(), Locale::EnUs);
        assert_eq!(catalog.available_locales().len(), 2);
    }

    #[test]
    fn resource_backed_catalog_bootstraps_and_reloads_from_disk() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new("test").with_resource(crate::TextResource::new(
            "en_US.json",
            r#"{"greeting":"hello"}"#,
        ));

        let catalog = ResourceBackedCatalog::bootstrap(
            temp.path(),
            &manifest,
            Locale::EnUs,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get(Locale::EnUs, "greeting"),
            Some("hello".to_string())
        );

        std::fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hi"}"#)
            .expect("rewrite locale file");
        catalog.reload().expect("reload catalog");

        assert_eq!(
            catalog.get(Locale::EnUs, "greeting"),
            Some("hi".to_string())
        );
    }
}
