mod catalog_error;
mod global_catalog;
mod i18n;
#[doc(hidden)]
pub mod lazy_catalog;
mod locale_selection;

pub use catalog_error::{CatalogInitError, CatalogLocaleError, CliLocaleError};
pub use global_catalog::GlobalCatalog;
pub use i18n::{
    CatalogBootstrapCleanupError, ResourceCatalogError, bootstrap_i18n_catalog,
    load_i18n_catalog_from_directory, reload_i18n_catalog_from_directory,
};
#[deprecated(
    since = "0.1.0",
    note = "LazyCatalog blocks threads during first initialization; prefer load/bootstrap helpers plus GlobalCatalog for runtime-facing handles."
)]
#[allow(deprecated)]
pub use lazy_catalog::LazyCatalog;
pub use locale_selection::{resolve_locale_from_argv, resolve_locale_from_cli_args};

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static CWD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    pub(crate) struct CurrentDirGuard {
        _lock: MutexGuard<'static, ()>,
        original: PathBuf,
    }

    impl CurrentDirGuard {
        pub(crate) fn new() -> Self {
            Self {
                _lock: CWD_LOCK.lock().unwrap_or_else(|poison| poison.into_inner()),
                original: std::env::current_dir().expect("capture cwd"),
            }
        }

        pub(crate) fn set(&self, path: &Path) {
            std::env::set_current_dir(path).expect("set cwd");
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original).expect("restore cwd");
        }
    }
}
