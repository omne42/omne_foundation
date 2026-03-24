#[cfg(any(feature = "i18n", feature = "prompts", test))]
mod bootstrap_lock;
mod data_root;
#[cfg(any(feature = "i18n", feature = "prompts"))]
mod lazy_state;
#[cfg(any(feature = "i18n", feature = "prompts", test))]
mod resource_bootstrap;
mod resource_path;
mod secure_fs;
mod text_directory;
mod text_resource;

#[cfg(feature = "i18n")]
mod i18n;
#[cfg(feature = "prompts")]
mod prompts;

pub use data_root::{DataRootOptions, DataRootScope, ensure_data_root, resolve_data_root};
#[cfg(feature = "i18n")]
pub use i18n::{
    CatalogBootstrapCleanupError, CatalogInitError, CatalogLocaleError, LazyCatalog,
    ResourceCatalogError, bootstrap_i18n_catalog,
};
#[cfg(feature = "i18n")]
pub use i18n_kit::{
    Catalog, DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy, Locale, ResolveLocaleError,
    TemplateArg, TranslationCatalog, TranslationResolution,
};
#[cfg(feature = "prompts")]
pub use prompts::{
    LazyPromptDirectory, PromptBootstrapCleanupError, PromptDirectoryError,
    bootstrap_prompt_directory,
};
pub use text_directory::TextDirectory;
pub use text_resource::{ResourceManifest, TextResource};

#[cfg(all(test, feature = "i18n"))]
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
