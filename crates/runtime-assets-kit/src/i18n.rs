use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use omne_systems_fs_primitives::MissingRootPolicy;

use i18n_kit::{
    Catalog, DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy, Locale, ResolveLocaleError,
    TemplateArg, resolve_locale_from_argv, resolve_locale_from_cli_args,
};

use crate::bootstrap_lock::lock_bootstrap_transaction;
use crate::lazy_state::{LazyInitError, LazyValue};
use crate::resource_bootstrap::{bootstrap_text_resources, rollback_created_resources};
use crate::resource_path::materialize_resource_root;
use crate::secure_fs::{MAX_TEXT_DIRECTORY_TOTAL_BYTES, SecureRoot};
use crate::text_resource::{ResourceManifest, manifest_resource_paths};

#[derive(Debug)]
pub enum ResourceCatalogError {
    Bootstrap(io::Error),
    Load(DynamicCatalogError),
}

impl Display for ResourceCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap(error) => write!(f, "bootstrap text resources: {error}"),
            Self::Load(error) => write!(f, "load i18n catalog: {error}"),
        }
    }
}

impl std::error::Error for ResourceCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bootstrap(error) => Some(error),
            Self::Load(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub struct CatalogBootstrapCleanupError {
    load: DynamicCatalogError,
    rollback: io::Error,
}

impl CatalogBootstrapCleanupError {
    #[must_use]
    pub fn load_error(&self) -> &DynamicCatalogError {
        &self.load
    }

    #[must_use]
    pub fn rollback_error(&self) -> &io::Error {
        &self.rollback
    }
}

impl Display for CatalogBootstrapCleanupError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "catalog load error: {}; rollback failed: {}",
            self.load, self.rollback
        )
    }
}

impl StdError for CatalogBootstrapCleanupError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.load)
    }
}

fn bootstrap_i18n_catalog_with_loader<L>(
    root: &Path,
    manifest: &ResourceManifest,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
    load: L,
) -> Result<DynamicJsonCatalog, ResourceCatalogError>
where
    L: FnOnce(
        &Path,
        &[String],
        Locale,
        FallbackStrategy,
    ) -> Result<DynamicJsonCatalog, DynamicCatalogError>,
{
    let root = materialize_resource_root(root).map_err(ResourceCatalogError::Bootstrap)?;
    validate_catalog_manifest(manifest, default_locale, fallback_strategy)
        .map_err(ResourceCatalogError::Load)?;
    let resource_paths = manifest_resource_paths(manifest);
    let _bootstrap_transaction =
        lock_bootstrap_transaction(&root).map_err(ResourceCatalogError::Bootstrap)?;
    let report =
        bootstrap_text_resources(&root, manifest).map_err(ResourceCatalogError::Bootstrap)?;
    match load(&root, &resource_paths, default_locale, fallback_strategy) {
        Ok(catalog) => Ok(catalog),
        Err(error) => {
            if let Err(rollback_error) = rollback_created_resources(&report) {
                return Err(ResourceCatalogError::Bootstrap(
                    catalog_bootstrap_cleanup_error(error, rollback_error),
                ));
            }
            Err(ResourceCatalogError::Load(error))
        }
    }
}

/// Bootstraps catalog resources under `root` and then rebuilds the catalog
/// from the managed files on disk.
///
/// Concurrent bootstrap attempts are serialized per materialized root, both
/// within the current process and across cooperating local processes that
/// resolve the same lock directory, so that rollback from one attempt cannot
/// invalidate another attempt's load.
pub fn bootstrap_i18n_catalog(
    root: impl AsRef<Path>,
    manifest: &ResourceManifest,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<DynamicJsonCatalog, ResourceCatalogError> {
    bootstrap_i18n_catalog_with_loader(
        root.as_ref(),
        manifest,
        default_locale,
        fallback_strategy,
        load_catalog_from_resource_files,
    )
}

fn catalog_bootstrap_cleanup_error(load: DynamicCatalogError, rollback: io::Error) -> io::Error {
    io::Error::new(
        rollback.kind(),
        CatalogBootstrapCleanupError { load, rollback },
    )
}

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

impl From<ResourceCatalogError> for CatalogInitError {
    fn from(error: ResourceCatalogError) -> Self {
        Self::new(error)
    }
}

#[derive(Debug, Clone)]
pub enum CatalogLocaleError {
    Initialization(CatalogInitError),
    Resolve(ResolveLocaleError),
}

impl Display for CatalogLocaleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialization(error) => Display::fmt(error, f),
            Self::Resolve(error) => Display::fmt(error, f),
        }
    }
}

impl StdError for CatalogLocaleError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Initialization(error) => Some(error),
            Self::Resolve(error) => Some(error),
        }
    }
}

/// Lazily initializes a shared catalog.
///
/// Concurrent access while the initializer is already running waits for the
/// initializer to finish and then observes the settled result. Recursive
/// initialization from the same thread is still rejected. Initializers must
/// not block on other threads or tasks that might re-enter the same catalog;
/// cross-thread cycles are a caller bug and may deadlock.
pub struct LazyCatalog {
    inner: LazyValue<dyn Catalog, CatalogInitError>,
    initializer: Box<dyn Fn() -> Result<Arc<dyn Catalog>, CatalogInitError> + Send + Sync>,
}

impl LazyCatalog {
    pub fn new<I>(initializer: I) -> Self
    where
        I: Fn() -> Result<Arc<dyn Catalog>, CatalogInitError> + Send + Sync + 'static,
    {
        Self {
            inner: LazyValue::new(),
            initializer: Box::new(initializer),
        }
    }

    pub fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, CatalogLocaleError> {
        self.with_catalog(|catalog| catalog.try_resolve_locale(requested))
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
            .map_err(CatalogLocaleError::Resolve)
    }

    pub fn resolve_locale_from_argv(
        &self,
        argv: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), CatalogLocaleError> {
        self.with_catalog(|catalog| resolve_locale_from_argv(catalog, argv, env_var))
            .map_err(CatalogLocaleError::Initialization)?
            .map_err(CatalogLocaleError::Resolve)
    }

    pub fn initialize(&self) -> Result<(), CatalogInitError> {
        self.with_catalog(|_| ())
    }

    pub fn try_render(
        &self,
        locale: Locale,
        key: &str,
        args: &[TemplateArg<'_>],
    ) -> Result<String, CatalogInitError> {
        self.with_catalog(|catalog| catalog.render_text(locale, key, args))
    }

    fn with_catalog<T>(&self, f: impl FnOnce(&dyn Catalog) -> T) -> Result<T, CatalogInitError> {
        let catalog = self
            .inner
            .get_or_init(|| (self.initializer)())
            .map_err(shared_lazy_catalog_error)?;
        Ok(f(catalog.as_ref()))
    }
}

fn shared_lazy_catalog_error(error: LazyInitError<CatalogInitError>) -> CatalogInitError {
    match error {
        LazyInitError::Inner(error) => shared_catalog_init_error(error),
        LazyInitError::ReentrantInitialization => {
            CatalogInitError::new(ReentrantCatalogInitialization)
        }
    }
}

fn shared_catalog_init_error(error: Arc<CatalogInitError>) -> CatalogInitError {
    error.as_ref().clone()
}

#[derive(Debug)]
struct ReentrantCatalogInitialization;

impl Display for ReentrantCatalogInitialization {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("reentrant catalog initialization")
    }
}

impl StdError for ReentrantCatalogInitialization {}

fn validate_catalog_manifest(
    manifest: &ResourceManifest,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<(), DynamicCatalogError> {
    DynamicJsonCatalog::from_locale_sources(
        manifest.resources().iter().map(|resource| {
            (
                PathBuf::from(resource.relative_path()),
                resource.contents().to_owned(),
            )
        }),
        default_locale,
        fallback_strategy,
    )
    .map(|_| ())
}

fn load_catalog_from_resource_files(
    root: &Path,
    resource_paths: &[String],
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<DynamicJsonCatalog, DynamicCatalogError> {
    let sources = load_catalog_resource_sources(root, resource_paths)?;
    DynamicJsonCatalog::from_locale_sources(sources, default_locale, fallback_strategy)
}

fn load_catalog_resource_sources(
    root: &Path,
    resource_paths: &[String],
) -> Result<Vec<(PathBuf, String)>, DynamicCatalogError> {
    let root = materialize_resource_root(root)?;
    let root = SecureRoot::open(&root, MissingRootPolicy::Error)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("resource root does not exist: {}", root.display()),
        )
    })?;

    let mut sources = Vec::with_capacity(resource_paths.len());
    let mut total_bytes = 0usize;
    for relative_path in resource_paths {
        let contents = root.read_file_to_string(relative_path)?;
        total_bytes = total_bytes.saturating_add(contents.len());
        if total_bytes > MAX_TEXT_DIRECTORY_TOTAL_BYTES {
            return Err(DynamicCatalogError::CatalogTooLarge {
                bytes: total_bytes,
                max_bytes: MAX_TEXT_DIRECTORY_TOTAL_BYTES,
            });
        }
        sources.push((PathBuf::from(relative_path), contents));
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secure_fs::MAX_TEXT_RESOURCE_BYTES;
    use crate::test_support::CurrentDirGuard;
    use i18n_kit::{TranslationCatalog, TranslationResolution};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, LazyLock, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

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
    fn resource_backed_catalog_rebuilds_snapshot_from_current_disk_state() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let first = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            first.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );

        std::fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hi"}"#)
            .expect("rewrite locale file");
        let second = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("rebuild catalog");

        assert_eq!(
            second.get_text(Locale::EN_US, "greeting"),
            Some("hi".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_failed_rebuild_keeps_previous_snapshot() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );

        std::fs::write(temp.path().join("en_US.json"), "{").expect("write invalid locale");
        let error = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid json rebuild should fail");
        assert!(matches!(
            error,
            ResourceCatalogError::Load(DynamicCatalogError::LocaleSourceJson { .. })
        ));
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_rebuild_rejects_oversized_locale_before_parsing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");

        std::fs::write(
            temp.path().join("en_US.json"),
            vec![b'x'; MAX_TEXT_RESOURCE_BYTES + 1],
        )
        .expect("write oversized locale");

        let error = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("oversized locale rebuild should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::Io(error)) = error else {
            panic!("expected io error for oversized locale file");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds size limit"));
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_rejects_catalogs_that_exceed_total_size_limit() {
        let temp = TempDir::new().expect("temp dir");
        let locales = [
            "en_US", "fr_FR", "de_DE", "es_ES", "it_IT", "ja_JP", "ko_KR", "pt_BR", "zh_CN",
        ];
        let mut manifest = ResourceManifest::new();
        for locale in locales {
            manifest = manifest.with_resource(
                crate::TextResource::new(
                    format!("{locale}.json"),
                    format!(r#"{{"greeting":"{}"}}"#, "x".repeat(950_000)),
                )
                .expect("valid resource"),
            );
        }

        let error = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("oversized catalog should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::CatalogTooLarge { bytes, max_bytes }) =
            error
        else {
            panic!("expected load total size limit error");
        };
        assert!(bytes > max_bytes);
    }

    #[test]
    fn resource_backed_catalog_rebuilds_from_absolute_root_across_cwd_changes() {
        let cwd = CurrentDirGuard::new();
        let temp = TempDir::new().expect("temp dir");
        let workspace_a = temp.path().join("workspace_a");
        let workspace_b = temp.path().join("workspace_b");
        let root = workspace_a.join("catalog");
        std::fs::create_dir_all(&workspace_a).expect("mkdir workspace_a");
        std::fs::create_dir_all(&workspace_b).expect("mkdir workspace_b");
        cwd.set(&workspace_a);

        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );
        let catalog = bootstrap_i18n_catalog(
            PathBuf::from("catalog"),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");

        cwd.set(&workspace_b);
        std::fs::write(root.join("en_US.json"), r#"{"greeting":"hi"}"#)
            .expect("rewrite locale file");
        let rebuilt =
            bootstrap_i18n_catalog(&root, &manifest, Locale::EN_US, FallbackStrategy::Both)
                .expect("rebuild catalog");

        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(
            rebuilt.get_text(Locale::EN_US, "greeting"),
            Some("hi".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_loads_nested_locale_files() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("i18n/en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(catalog.available_locales(), vec![Locale::EN_US]);
    }

    #[test]
    fn resource_backed_catalog_ignores_unmanaged_root_json_files() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("notes.json"), r#"{"ignore":"me"}"#)
            .expect("write unrelated json");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("i18n/en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(catalog.available_locales(), vec![Locale::EN_US]);
    }

    #[test]
    fn resource_backed_catalog_errors_when_default_locale_is_missing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("zh_CN.json", r#"{"greeting":"nihao"}"#)
                .expect("valid resource"),
        );

        let err = match bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        ) {
            Ok(_) => panic!("missing default locale should fail"),
            Err(err) => err,
        };
        let ResourceCatalogError::Load(DynamicCatalogError::MissingDefaultLocale(locale)) = err
        else {
            panic!("expected missing default locale load error");
        };
        assert_eq!(locale, Locale::EN_US);
    }

    #[test]
    fn resource_backed_catalog_bootstrap_rejects_invalid_manifest_without_writing_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let invalid_manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("i18n/en_US.json", "{").expect("valid resource path"),
        );

        let err = bootstrap_i18n_catalog(
            &root,
            &invalid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid catalog json should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::LocaleSourceJson { path, .. }) = err
        else {
            panic!("expected json load error");
        };
        assert_eq!(path, "i18n/en_US.json");
        assert!(!root.exists());

        let valid_manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("i18n/en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );
        let catalog = bootstrap_i18n_catalog(
            &root,
            &valid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("second bootstrap should recover");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_bootstrap_rejects_invalid_template_without_writing_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let invalid_manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("i18n/en_US.json", r#"{"greeting":"hello {name"}"#)
                .expect("valid resource path"),
        );

        let err = bootstrap_i18n_catalog(
            &root,
            &invalid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid catalog template should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::LocaleSourceJson { path, error }) = err
        else {
            panic!("expected template validation load error");
        };
        assert_eq!(path, "i18n/en_US.json");
        assert!(
            error
                .to_string()
                .contains("invalid catalog template for greeting: unclosed placeholder")
        );
        assert!(!root.exists());
    }

    #[test]
    fn resource_backed_catalog_rejects_invalid_locale_file_name_without_writing_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let invalid_manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("i18n/not-a-locale.txt", r#"{"greeting":"hello"}"#)
                .expect("valid resource path"),
        );

        let err = bootstrap_i18n_catalog(
            &root,
            &invalid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid locale file name should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::InvalidLocaleFileName(path)) = err
        else {
            panic!("expected invalid locale file name load error");
        };
        assert_eq!(path, "i18n/not-a-locale.txt");
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn resource_backed_catalog_rebuild_rejects_symlinked_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let backup = temp.path().join("catalog_real");
        let outside = TempDir::new().expect("outside dir");
        let manifest = ResourceManifest::new().with_resource(
            crate::TextResource::new("en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );
        let catalog =
            bootstrap_i18n_catalog(&root, &manifest, Locale::EN_US, FallbackStrategy::Both)
                .expect("bootstrap catalog");

        std::fs::rename(&root, &backup).expect("move root aside");
        std::fs::write(
            outside.path().join("en_US.json"),
            r#"{"greeting":"outside"}"#,
        )
        .expect("write outside locale");
        symlink(outside.path(), &root).expect("symlink root");

        let err = bootstrap_i18n_catalog(&root, &manifest, Locale::EN_US, FallbackStrategy::Both)
            .expect_err("symlinked root should fail");
        let ResourceCatalogError::Bootstrap(error) = err else {
            panic!("expected bootstrap io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn lazy_catalog_rejects_reentrant_initialization() {
        let error = REENTRANT_CATALOG
            .initialize()
            .expect_err("reentrant init should fail");
        assert_eq!(error.to_string(), "reentrant catalog initialization");
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
    fn catalog_bootstrap_cleanup_error_preserves_both_failures() {
        let load_error = DynamicJsonCatalog::from_locale_sources(
            [(PathBuf::from("en_US.json"), "{".to_string())],
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid catalog json should fail");
        let error =
            catalog_bootstrap_cleanup_error(load_error, io::Error::other("rollback failed"));

        let cleanup = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<CatalogBootstrapCleanupError>())
            .expect("wrapped cleanup error");
        assert!(matches!(
            cleanup.load_error(),
            DynamicCatalogError::LocaleSourceJson { .. }
        ));
        assert_eq!(cleanup.rollback_error().to_string(), "rollback failed");
        assert!(matches!(
            cleanup
                .source()
                .expect("load source")
                .downcast_ref::<DynamicCatalogError>(),
            Some(DynamicCatalogError::LocaleSourceJson { .. })
        ));
        assert!(cleanup.to_string().contains("catalog load error:"));
        assert!(
            cleanup
                .to_string()
                .contains("rollback failed: rollback failed")
        );
    }
}
