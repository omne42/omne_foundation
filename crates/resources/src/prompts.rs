use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::text_directory::{GlobalTextDirectory, TextDirectory};
use crate::text_resource::{ResourceManifest, bootstrap_text_resources};

pub struct ResourceBackedPromptCatalog {
    root: PathBuf,
    inner: TextDirectory,
}

impl ResourceBackedPromptCatalog {
    pub fn bootstrap(
        root: impl Into<PathBuf>,
        manifest: &ResourceManifest,
    ) -> Result<Self, std::io::Error> {
        let root = root.into();
        bootstrap_text_resources(&root, manifest)?;
        let inner = TextDirectory::load(&root)?;
        Ok(Self { root, inner })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn reload(&mut self) -> Result<(), std::io::Error> {
        self.inner = TextDirectory::load(&self.root)?;
        Ok(())
    }

    #[must_use]
    pub fn directory(&self) -> &TextDirectory {
        &self.inner
    }
}

pub struct LazyPromptCatalog {
    inner: GlobalTextDirectory,
    initialized: OnceLock<Result<(), std::io::Error>>,
    initializer: fn() -> Result<TextDirectory, std::io::Error>,
}

impl LazyPromptCatalog {
    pub const fn new(initializer: fn() -> Result<TextDirectory, std::io::Error>) -> Self {
        Self {
            inner: GlobalTextDirectory::new(),
            initialized: OnceLock::new(),
            initializer,
        }
    }

    pub fn replace(&self, directory: TextDirectory) {
        self.inner.replace(directory);
        let _ = self.initialized.set(Ok(()));
    }

    fn ensure_initialized(&self) {
        let _ = self.initialized.get_or_init(|| {
            let directory = (self.initializer)()?;
            self.inner.replace(directory);
            Ok(())
        });
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        self.ensure_initialized();
        self.inner.get(key)
    }

    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.ensure_initialized();
        self.inner.keys()
    }
}
