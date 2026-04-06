use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use i18n_kit::{Catalog, Locale, TranslationCatalog, TranslationResolution};
use text_assets_kit::SharedRuntimeHandle;

use crate::catalog_error::{CatalogInitError, CatalogLocaleError};
use crate::{resolve_locale_from_argv, resolve_locale_from_cli_args};

/// Thread-safe runtime catalog handle.
///
/// Readers clone the current `Arc<dyn Catalog>` behind a shared `RwLock` and
/// run catalog code after the lock has been released. Replacements only
/// contend on swapping the pointer, so slow reads do not stall hot reloads.
///
/// Translation lookups on an uninitialized handle behave like misses. Runtime
/// APIs that require a concrete catalog report `CatalogInitError` instead of
/// pretending the handle is itself a pure `Catalog`.
pub struct GlobalCatalog {
    default_locale: Locale,
    inner: SharedRuntimeHandle<dyn Catalog>,
}

impl GlobalCatalog {
    #[must_use]
    pub const fn new(default_locale: Locale) -> Self {
        Self {
            default_locale,
            inner: SharedRuntimeHandle::new(),
        }
    }

    pub fn replace<C>(&self, catalog: C)
    where
        C: Catalog + 'static,
    {
        self.inner.replace_shared(Arc::new(catalog));
    }

    #[must_use]
    pub fn default_locale(&self) -> Locale {
        self.current_catalog()
            .map(|catalog| catalog.default_locale())
            .unwrap_or(self.default_locale)
    }

    #[must_use]
    pub fn available_locales(&self) -> Vec<Locale> {
        self.current_catalog()
            .map(|catalog| catalog.available_locales())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn locale_enabled(&self, locale: Locale) -> bool {
        self.current_catalog()
            .map(|catalog| catalog.locale_enabled(locale))
            .unwrap_or(false)
    }

    pub fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, CatalogLocaleError> {
        self.with_catalog(|catalog| catalog.try_resolve_locale(requested))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Resolve)
    }

    pub fn resolve_environment_locale(
        &self,
        requested: &str,
    ) -> Result<Locale, CatalogLocaleError> {
        self.with_catalog(|catalog| catalog.try_resolve_environment_locale(requested))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Resolve)
    }

    pub fn resolve_locale_from_cli_args(
        &self,
        args: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), CatalogLocaleError> {
        self.with_catalog(|catalog| resolve_locale_from_cli_args(catalog, args, env_var))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Cli)
    }

    pub fn resolve_locale_from_argv(
        &self,
        argv: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), CatalogLocaleError> {
        self.with_catalog(|catalog| resolve_locale_from_argv(catalog, argv, env_var))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Cli)
    }

    pub fn with_catalog<T>(
        &self,
        f: impl FnOnce(&dyn Catalog) -> T,
    ) -> Result<T, CatalogInitError> {
        let catalog = self
            .current_catalog()
            .ok_or_else(|| CatalogInitError::new(UninitializedCatalog))?;
        Ok(f(catalog.as_ref()))
    }

    fn current_catalog(&self) -> Option<Arc<dyn Catalog>> {
        self.inner.current()
    }
}

impl TranslationCatalog for GlobalCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        self.current_catalog()
            .map(|catalog| catalog.resolve_shared(locale, key))
            .unwrap_or(TranslationResolution::Missing)
    }
}

#[derive(Debug)]
struct UninitializedCatalog;

impl Display for UninitializedCatalog {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("catalog not initialized")
    }
}

impl StdError for UninitializedCatalog {}

#[cfg(test)]
mod tests {
    use super::*;
    use i18n_kit::{ResolveLocaleError, StaticJsonCatalog, StaticJsonLocale, TemplateArg};
    use std::sync::{Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn global_catalog_uses_installed_catalog() {
        assert_send_sync::<GlobalCatalog>();

        static GLOBAL: GlobalCatalog = GlobalCatalog::new(Locale::EN_US);
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"hello":"hello"}"#,
        )];
        let catalog = StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog");

        GLOBAL.replace(catalog);
        assert_eq!(
            GLOBAL.get_text(Locale::EN_US, "hello"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn global_catalog_uninitialized_lookup_is_a_miss() {
        let catalog = GlobalCatalog::new(Locale::EN_US);

        assert_eq!(catalog.default_locale(), Locale::EN_US);
        assert_eq!(catalog.available_locales(), Vec::<Locale>::new());
        assert!(!catalog.locale_enabled(Locale::EN_US));
        assert_eq!(catalog.get_text(Locale::EN_US, "hello"), None);
        assert_eq!(
            catalog
                .with_catalog(|catalog| catalog.default_locale())
                .unwrap_err()
                .to_string(),
            "catalog not initialized"
        );
        assert!(matches!(
            catalog.resolve_locale(None),
            Err(CatalogLocaleError::Initialization(_))
        ));
        assert!(matches!(
            catalog.resolve_locale_from_argv(vec!["cmd".to_string()], "APP_LOCALE"),
            Err(CatalogLocaleError::Initialization(_))
        ));
        assert_eq!(
            catalog.render_text(
                Locale::EN_US,
                "hello.{name}",
                &[TemplateArg::new("name", "Alice")],
            ),
            "hello.{name}".to_string()
        );
        assert!(matches!(
            catalog.resolve_shared(Locale::EN_US, "hello"),
            TranslationResolution::Missing
        ));
    }

    #[test]
    fn global_catalog_initialized_missing_render_returns_raw_key() {
        let catalog = GlobalCatalog::new(Locale::EN_US);
        let sources = [StaticJsonLocale::new(Locale::EN_US, true, "{}")];
        let inner = StaticJsonCatalog::try_new(Locale::EN_US, &sources).expect("valid catalog");

        catalog.replace(inner);

        assert_eq!(
            catalog.render_text(
                Locale::EN_US,
                "hello.{name}",
                &[TemplateArg::new("name", "Alice")],
            ),
            "hello.{name}"
        );
    }

    #[test]
    fn global_catalog_replace_does_not_wait_for_inflight_reads() {
        struct BlockingCatalog {
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }

        impl TranslationCatalog for BlockingCatalog {
            fn resolve_shared(&self, _locale: Locale, _key: &str) -> TranslationResolution {
                self.entered.send(()).expect("signal reader entered");
                self.release
                    .lock()
                    .expect("lock release receiver")
                    .recv()
                    .expect("release reader");
                TranslationResolution::Exact(Arc::<str>::from("slow"))
            }
        }

        impl Catalog for BlockingCatalog {
            fn default_locale(&self) -> Locale {
                Locale::EN_US
            }

            fn available_locales(&self) -> Vec<Locale> {
                vec![Locale::EN_US]
            }

            fn locale_enabled(&self, locale: Locale) -> bool {
                locale == Locale::EN_US
            }
        }

        let catalog = Arc::new(GlobalCatalog::new(Locale::EN_US));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        catalog.replace(BlockingCatalog {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        });

        let reading_catalog = Arc::clone(&catalog);
        let handle = thread::spawn(move || reading_catalog.get_text(Locale::EN_US, "hello"));

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reader should enter old catalog");

        let replacement = StaticJsonCatalog::try_new(
            Locale::EN_US,
            &[StaticJsonLocale::new(
                Locale::EN_US,
                true,
                r#"{"hello":"fresh"}"#,
            )],
        )
        .expect("valid replacement");
        catalog.replace(replacement);

        assert_eq!(
            catalog.get_text(Locale::EN_US, "hello"),
            Some("fresh".to_string())
        );

        release_tx.send(()).expect("release old reader");
        assert_eq!(
            handle.join().expect("join old reader"),
            Some("slow".to_string())
        );
    }

    #[test]
    fn global_catalog_try_resolve_locale_keeps_error_kind_under_replacement() {
        let catalog = Arc::new(GlobalCatalog::new(Locale::EN_US));
        let old = StaticJsonCatalog::try_new(
            Locale::EN_US,
            &[StaticJsonLocale::new(
                Locale::EN_US,
                true,
                r#"{"hello":"hello"}"#,
            )],
        )
        .expect("valid old catalog");
        catalog.replace(old);

        let resolving_catalog = Arc::clone(&catalog);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            struct BlockingResolveCatalog {
                entered: mpsc::Sender<()>,
                release: Mutex<mpsc::Receiver<()>>,
            }

            impl TranslationCatalog for BlockingResolveCatalog {
                fn resolve_shared(&self, _locale: Locale, _key: &str) -> TranslationResolution {
                    TranslationResolution::Missing
                }
            }

            impl Catalog for BlockingResolveCatalog {
                fn try_resolve_locale(
                    &self,
                    requested: Option<&str>,
                ) -> Result<Locale, ResolveLocaleError> {
                    self.entered.send(()).expect("signal resolve entered");
                    self.release
                        .lock()
                        .expect("lock release receiver")
                        .recv()
                        .expect("release resolver");
                    match requested {
                        Some(requested) => Err(ResolveLocaleError::UnknownLocale {
                            requested: requested.to_string(),
                        }),
                        None => Ok(Locale::EN_US),
                    }
                }

                fn default_locale(&self) -> Locale {
                    Locale::EN_US
                }

                fn available_locales(&self) -> Vec<Locale> {
                    vec![Locale::EN_US]
                }

                fn locale_enabled(&self, locale: Locale) -> bool {
                    locale == Locale::EN_US
                }
            }

            resolving_catalog.replace(BlockingResolveCatalog {
                entered: entered_tx,
                release: Mutex::new(release_rx),
            });
            resolving_catalog.resolve_locale(Some("fr_FR"))
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("resolver should enter old catalog");

        let replacement = StaticJsonCatalog::try_new(
            Locale::EN_US,
            &[StaticJsonLocale::new(
                Locale::EN_US,
                true,
                r#"{"hello":"fresh"}"#,
            )],
        )
        .expect("valid replacement");
        catalog.replace(replacement);

        release_tx.send(()).expect("release resolver");
        let error = handle
            .join()
            .expect("join resolver")
            .expect_err("old resolver should still fail");
        let CatalogLocaleError::Resolve(error) = error else {
            panic!("expected locale resolution error");
        };
        assert!(matches!(
            error,
            ResolveLocaleError::UnknownLocale { requested } if requested == "fr_FR"
        ));
    }
}
