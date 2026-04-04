#![allow(deprecated)]

use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use i18n_kit::{Catalog, Locale, TemplateArg};
#[allow(deprecated)]
use text_assets_kit::{LazyInitError, LazyValue};

use crate::catalog_error::{CatalogInitError, CatalogLocaleError};
use crate::{resolve_locale_from_argv, resolve_locale_from_cli_args};

/// Legacy blocking catalog shim.
///
/// Concurrent access while the initializer is already running waits for the
/// initializer to finish and then observes the settled result. Recursive
/// initialization from the same thread is still rejected, and thread-level
/// cross-thread wait cycles are rejected before they can deadlock. Because
/// this path uses a blocking `Condvar`-based primitive, async runtime
/// boundaries should still prefer eager load/bootstrap plus `GlobalCatalog`.
#[deprecated(
    since = "0.1.0",
    note = "LazyCatalog is a blocking compatibility shim; prefer GlobalCatalog plus eager load/bootstrap for runtime-facing catalog access"
)]
#[allow(deprecated)]
pub struct LazyCatalog {
    inner: LazyValue<dyn Catalog, CatalogInitError>,
    initializer: Box<dyn Fn() -> Result<Arc<dyn Catalog>, CatalogInitError> + Send + Sync>,
}

#[allow(deprecated)]
impl LazyCatalog {
    #[allow(deprecated)]
    pub fn new<I>(initializer: I) -> Self
    where
        I: Fn() -> Result<Arc<dyn Catalog>, CatalogInitError> + Send + Sync + 'static,
    {
        Self {
            inner: LazyValue::new(),
            initializer: Box::new(initializer),
        }
    }

    /// Eagerly installs a catalog snapshot.
    ///
    /// This wakes threads waiting on initialization. An initializer already
    /// running on another thread is not cancelled; if it later returns, the
    /// replacement remains the visible catalog snapshot.
    pub fn replace<C>(&self, catalog: C)
    where
        C: Catalog + 'static,
    {
        self.inner.set(Arc::new(catalog));
    }

    #[allow(deprecated)]
    pub fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, CatalogLocaleError> {
        self.with_catalog(|catalog| catalog.try_resolve_locale(requested))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Resolve)
    }

    #[allow(deprecated)]
    pub fn resolve_locale_from_cli_args(
        &self,
        args: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), CatalogLocaleError> {
        self.with_catalog(|catalog| resolve_locale_from_cli_args(catalog, args, env_var))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Cli)
    }

    #[allow(deprecated)]
    pub fn resolve_locale_from_argv(
        &self,
        argv: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), CatalogLocaleError> {
        self.with_catalog(|catalog| resolve_locale_from_argv(catalog, argv, env_var))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Cli)
    }

    #[allow(deprecated)]
    pub fn initialize(&self) -> Result<(), CatalogInitError> {
        self.with_catalog(|_| ())
    }

    #[allow(deprecated)]
    pub fn try_render(
        &self,
        locale: Locale,
        key: &str,
        args: &[TemplateArg<'_>],
    ) -> Result<String, CatalogInitError> {
        self.with_catalog(|catalog| catalog.render_text(locale, key, args))
    }

    #[allow(deprecated)]
    pub fn with_catalog<T>(
        &self,
        f: impl FnOnce(&dyn Catalog) -> T,
    ) -> Result<T, CatalogInitError> {
        let catalog = self
            .inner
            .get_or_init(|| (self.initializer)())
            .map_err(shared_lazy_catalog_error)?;
        Ok(f(catalog.as_ref()))
    }
}

#[allow(deprecated)]
fn shared_lazy_catalog_error(error: LazyInitError<CatalogInitError>) -> CatalogInitError {
    match error {
        LazyInitError::Inner(error) => error.as_ref().clone(),
        LazyInitError::ReentrantInitialization => {
            CatalogInitError::new(ReentrantCatalogInitialization)
        }
        LazyInitError::SameThreadInitializationConflict => {
            CatalogInitError::new(SameThreadCatalogInitializationConflict)
        }
        LazyInitError::CrossThreadCycleDetected => {
            CatalogInitError::new(CrossThreadCatalogInitialization)
        }
    }
}

#[derive(Debug)]
struct ReentrantCatalogInitialization;

impl Display for ReentrantCatalogInitialization {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("reentrant catalog initialization")
    }
}

impl StdError for ReentrantCatalogInitialization {}

#[derive(Debug)]
struct SameThreadCatalogInitializationConflict;

impl Display for SameThreadCatalogInitializationConflict {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("same-thread catalog initialization conflict")
    }
}

impl StdError for SameThreadCatalogInitializationConflict {}

#[derive(Debug)]
struct CrossThreadCatalogInitialization;

impl Display for CrossThreadCatalogInitialization {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("cross-thread catalog initialization cycle detected")
    }
}

impl StdError for CrossThreadCatalogInitialization {}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use i18n_kit::{ResolveLocaleError, TranslationCatalog, TranslationResolution};
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, LazyLock, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use crate::CliLocaleError;

    #[derive(Debug)]
    struct TestCatalog;

    impl TranslationCatalog for TestCatalog {
        fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
            match (locale.as_str(), key) {
                ("en_US", "greeting") => TranslationResolution::Exact(Arc::<str>::from("hello")),
                ("fr_FR", "greeting") => TranslationResolution::Exact(Arc::<str>::from("bonjour")),
                _ => TranslationResolution::Missing,
            }
        }
    }

    impl Catalog for TestCatalog {
        fn default_locale(&self) -> Locale {
            Locale::EN_US
        }

        fn available_locales(&self) -> Vec<Locale> {
            vec![Locale::EN_US, Locale::parse("fr_FR").expect("fr_FR")]
        }

        fn locale_enabled(&self, locale: Locale) -> bool {
            matches!(locale.as_str(), "en_US" | "fr_FR")
        }
    }

    #[derive(Debug)]
    struct InitFailedError;

    impl Display for InitFailedError {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("init failed")
        }
    }

    impl StdError for InitFailedError {}

    #[derive(Debug)]
    struct InnerCatalogError;

    impl Display for InnerCatalogError {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("inner catalog error")
        }
    }

    impl StdError for InnerCatalogError {}

    fn failing_initializer() -> Result<Arc<dyn Catalog>, CatalogInitError> {
        Err(CatalogInitError::new(InitFailedError))
    }

    fn failing_initializer_with_source() -> Result<Arc<dyn Catalog>, CatalogInitError> {
        Err(CatalogInitError::new(io::Error::other(InnerCatalogError)))
    }

    fn lazy_get_text_shared(
        catalog: &LazyCatalog,
        locale: Locale,
        key: &str,
    ) -> Result<Option<Arc<str>>, CatalogInitError> {
        catalog.with_catalog(|catalog| catalog.get_text_shared(locale, key))
    }

    fn lazy_get_text(
        catalog: &LazyCatalog,
        locale: Locale,
        key: &str,
    ) -> Result<Option<String>, CatalogInitError> {
        catalog.with_catalog(|catalog| catalog.get_text(locale, key))
    }

    static REENTRANT_CATALOG: LazyLock<LazyCatalog> =
        LazyLock::new(|| LazyCatalog::new(reentrant_initializer));

    fn reentrant_initializer() -> Result<Arc<dyn Catalog>, CatalogInitError> {
        REENTRANT_CATALOG
            .initialize()
            .map(|()| Arc::new(TestCatalog) as Arc<dyn Catalog>)
    }

    #[test]
    fn lazy_catalog_reports_initializer_failure() {
        let catalog = LazyCatalog::new(failing_initializer);

        let error = lazy_get_text_shared(&catalog, Locale::EN_US, "greeting")
            .expect_err("init failure should surface");
        assert_eq!(error.to_string(), "init failed");
    }

    #[test]
    fn lazy_catalog_retries_after_initializer_failure() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let catalog = LazyCatalog::new({
            let attempts = Arc::clone(&attempts);
            move || {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(CatalogInitError::new(InitFailedError))
                } else {
                    Ok(Arc::new(TestCatalog) as Arc<dyn Catalog>)
                }
            }
        });

        let error = lazy_get_text_shared(&catalog, Locale::EN_US, "greeting")
            .expect_err("first init should fail");
        assert_eq!(error.to_string(), "init failed");

        assert_eq!(
            lazy_get_text_shared(&catalog, Locale::EN_US, "greeting")
                .expect("second init should retry"),
            Some(Arc::<str>::from("hello"))
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lazy_catalog_remains_uninitialized_while_retry_is_in_progress() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let catalog = Arc::new(LazyCatalog::new({
            let attempts = Arc::clone(&attempts);
            let release_rx = Arc::clone(&release_rx);
            move || {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(CatalogInitError::new(InitFailedError))
                } else {
                    entered_tx.send(()).expect("signal retry entered");
                    release_rx
                        .lock()
                        .expect("lock release channel")
                        .recv()
                        .expect("release retry");
                    Ok(Arc::new(TestCatalog) as Arc<dyn Catalog>)
                }
            }
        }));

        let error = lazy_get_text_shared(&catalog, Locale::EN_US, "greeting")
            .expect_err("first init should fail");
        assert_eq!(error.to_string(), "init failed");

        let retrying = Arc::clone(&catalog);
        let handle =
            thread::spawn(move || retrying.with_catalog(|catalog| catalog.available_locales()));
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("retry should start");

        release_tx.send(()).expect("release retry");
        let locales = handle
            .join()
            .expect("join retry thread")
            .expect("retry should succeed");
        assert_eq!(locales.len(), 2);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lazy_catalog_new_accepts_captured_runtime_state() {
        let shared_catalog: Arc<dyn Catalog> = Arc::new(TestCatalog);
        let catalog = LazyCatalog::new({
            let shared_catalog = Arc::clone(&shared_catalog);
            move || Ok(Arc::clone(&shared_catalog))
        });

        assert_eq!(
            lazy_get_text_shared(&catalog, Locale::EN_US, "greeting").expect("catalog value"),
            Some(Arc::<str>::from("hello"))
        );
    }

    #[test]
    fn lazy_catalog_replace_seeds_catalog_without_initializer() {
        let catalog = LazyCatalog::new(|| -> Result<Arc<dyn Catalog>, CatalogInitError> {
            panic!("initializer should not run after replace")
        });

        catalog.replace(TestCatalog);

        assert_eq!(
            lazy_get_text_shared(&catalog, Locale::EN_US, "greeting").expect("catalog value"),
            Some(Arc::<str>::from("hello"))
        );
    }

    #[test]
    fn lazy_catalog_replace_overrides_existing_snapshot() {
        struct ReplacedCatalog;

        impl TranslationCatalog for ReplacedCatalog {
            fn resolve_shared(&self, _locale: Locale, key: &str) -> TranslationResolution {
                match key {
                    "greeting" => TranslationResolution::Exact(Arc::<str>::from("hola")),
                    _ => TranslationResolution::Missing,
                }
            }
        }

        impl Catalog for ReplacedCatalog {
            fn default_locale(&self) -> Locale {
                Locale::EN_US
            }

            fn available_locales(&self) -> Vec<Locale> {
                vec![Locale::EN_US]
            }
        }

        let catalog = LazyCatalog::new(|| Ok(Arc::new(TestCatalog) as Arc<dyn Catalog>));
        assert_eq!(
            lazy_get_text(&catalog, Locale::EN_US, "greeting").expect("initial value"),
            Some("hello".to_string())
        );

        catalog.replace(ReplacedCatalog);

        assert_eq!(
            lazy_get_text(&catalog, Locale::EN_US, "greeting").expect("replaced value"),
            Some("hola".to_string())
        );
    }

    #[test]
    fn lazy_catalog_try_render_surfaces_initialization_failure() {
        let catalog = LazyCatalog::new(failing_initializer);

        let error = catalog
            .try_render(Locale::EN_US, "greeting", &[])
            .expect_err("init error should be preserved");
        assert_eq!(error.to_string(), "init failed");
    }

    #[test]
    fn lazy_catalog_with_catalog_preserves_initialization_failure() {
        let catalog = LazyCatalog::new(failing_initializer);

        let error = catalog
            .with_catalog(|catalog| catalog.default_locale())
            .expect_err("init error should be preserved");
        assert_eq!(error.to_string(), "init failed");
    }

    #[test]
    fn lazy_catalog_preserves_error_source_chain() {
        let catalog = LazyCatalog::new(failing_initializer_with_source);

        let error = lazy_get_text_shared(&catalog, Locale::EN_US, "greeting")
            .expect_err("init error should surface");
        assert_eq!(error.to_string(), "inner catalog error");
        let source = error.source().expect("wrapped source");
        assert_eq!(source.to_string(), "inner catalog error");
    }

    #[test]
    fn lazy_catalog_rejects_reentrant_initialization() {
        let error = REENTRANT_CATALOG
            .initialize()
            .expect_err("reentrant init should fail");
        assert_eq!(error.to_string(), "reentrant catalog initialization");
    }

    #[test]
    fn lazy_catalog_reports_same_thread_conflict() {
        let error = shared_lazy_catalog_error(LazyInitError::SameThreadInitializationConflict);
        assert_eq!(
            error.to_string(),
            "same-thread catalog initialization conflict"
        );
    }

    #[test]
    fn lazy_catalog_waits_for_concurrent_initialization() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let catalog = Arc::new(LazyCatalog::new({
            let release_rx = Arc::clone(&release_rx);
            move || {
                entered_tx.send(()).expect("signal initializer entered");
                release_rx
                    .lock()
                    .expect("lock release channel")
                    .recv()
                    .expect("release initializer");
                Ok(Arc::new(TestCatalog) as Arc<dyn Catalog>)
            }
        }));

        let initializing = Arc::clone(&catalog);
        let handle = thread::spawn(move || initializing.initialize());

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initializer should start");

        let waiting = Arc::clone(&catalog);
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            result_tx
                .send(lazy_get_text(&waiting, Locale::EN_US, "greeting"))
                .expect("publish concurrent result");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "concurrent access should wait for initialization to complete",
        );

        release_tx.send(()).expect("release initializer");
        handle
            .join()
            .expect("join initializer thread")
            .expect("initializer should succeed");
        waiter.join().expect("join waiting thread");

        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("concurrent access should complete")
                .expect("catalog should initialize successfully"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn lazy_catalog_reports_init_failure_from_accessors() {
        let catalog = LazyCatalog::new(failing_initializer);

        assert_eq!(
            catalog
                .with_catalog(|catalog| catalog.default_locale())
                .expect_err("init failure should surface")
                .to_string(),
            "init failed"
        );
        assert_eq!(
            catalog
                .with_catalog(|catalog| catalog.available_locales())
                .expect_err("init failure should surface")
                .to_string(),
            "init failed"
        );
        assert_eq!(
            catalog
                .with_catalog(|catalog| catalog.locale_enabled(Locale::EN_US))
                .expect_err("init failure should surface")
                .to_string(),
            "init failed"
        );
    }

    #[test]
    fn lazy_catalog_resolve_locale_reports_structured_errors() {
        let catalog = LazyCatalog::new(|| Ok(Arc::new(TestCatalog) as Arc<dyn Catalog>));

        let error = catalog
            .resolve_locale(Some("de_DE"))
            .expect_err("disabled locale should fail");
        let CatalogLocaleError::Resolve(ResolveLocaleError::LocaleNotEnabled { requested, .. }) =
            error
        else {
            panic!("expected structured resolve error");
        };
        assert_eq!(requested, "de_DE");
    }

    #[test]
    fn lazy_catalog_resolve_locale_from_cli_args_reports_cli_errors() {
        let catalog = LazyCatalog::new(|| Ok(Arc::new(TestCatalog) as Arc<dyn Catalog>));

        let error = catalog
            .resolve_locale_from_cli_args(vec!["--locale".to_string()], "APP_LOCALE")
            .expect_err("missing CLI locale value should fail");

        assert!(matches!(
            error,
            CatalogLocaleError::Cli(CliLocaleError::MissingValue {
                flag: "--lang/--locale"
            })
        ));
    }
}
