use std::io;
use std::path::Path;
use std::sync::Arc;

use text_assets_kit::{
    BootstrapLoadError, LazyInitError, LazyValue, ResourceManifest, TextDirectory,
    bootstrap_text_resources_then_load,
};

#[derive(Debug)]
pub struct PromptBootstrapCleanupError {
    load: io::Error,
    rollback: io::Error,
}

impl PromptBootstrapCleanupError {
    #[must_use]
    pub fn load_error(&self) -> &io::Error {
        &self.load
    }

    #[must_use]
    pub fn rollback_error(&self) -> &io::Error {
        &self.rollback
    }
}

impl std::fmt::Display for PromptBootstrapCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "prompt directory load error: {}; rollback failed: {}",
            self.load, self.rollback
        )
    }
}

impl std::error::Error for PromptBootstrapCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.load)
    }
}

fn bootstrap_prompt_directory_with_loader<L>(
    root: &Path,
    manifest: &ResourceManifest,
    load: L,
) -> Result<TextDirectory, io::Error>
where
    L: FnOnce(&Path, &[String]) -> io::Result<TextDirectory>,
{
    match bootstrap_text_resources_then_load(root, manifest, load) {
        Ok(directory) => Ok(directory),
        Err(BootstrapLoadError::Bootstrap(error) | BootstrapLoadError::Load(error)) => Err(error),
        Err(BootstrapLoadError::Rollback { load, rollback }) => {
            Err(prompt_bootstrap_cleanup_error(load, rollback))
        }
    }
}

/// Bootstraps prompt resources under `root` and then reloads the managed files
/// into a fresh snapshot.
///
/// Concurrent bootstrap attempts are serialized per materialized root, both
/// within the current process and across cooperating local processes that
/// resolve the same lock directory, so that rollback from one attempt cannot
/// invalidate another attempt's load.
pub fn bootstrap_prompt_directory(
    root: impl AsRef<Path>,
    manifest: &ResourceManifest,
) -> Result<TextDirectory, io::Error> {
    bootstrap_prompt_directory_with_loader(
        root.as_ref(),
        manifest,
        TextDirectory::load_resource_files,
    )
}

fn prompt_bootstrap_cleanup_error(load: io::Error, rollback: io::Error) -> io::Error {
    io::Error::new(
        rollback.kind(),
        PromptBootstrapCleanupError { load, rollback },
    )
}

/// Lazily initializes a shared prompt directory.
///
/// Concurrent access while initialization is in flight waits for the
/// initializer to finish and then observes the settled result. Initializers
/// must not block on other threads or tasks that might re-enter the same
/// prompt directory; cross-thread cycles are a caller bug and may deadlock.
pub struct LazyPromptDirectory {
    inner: LazyValue<TextDirectory, io::Error>,
    initializer: Box<dyn Fn() -> Result<TextDirectory, io::Error> + Send + Sync>,
}

#[derive(Debug, Clone)]
pub struct PromptDirectoryError(Arc<io::Error>);

impl PromptDirectoryError {
    fn new(error: Arc<io::Error>) -> Self {
        Self(error)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn kind(&self) -> io::ErrorKind {
        self.0.kind()
    }
}

impl std::fmt::Display for PromptDirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.0.as_ref(), f)
    }
}

impl std::error::Error for PromptDirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

impl LazyPromptDirectory {
    pub fn new<I>(initializer: I) -> Self
    where
        I: Fn() -> Result<TextDirectory, io::Error> + Send + Sync + 'static,
    {
        Self {
            inner: LazyValue::new(),
            initializer: Box::new(initializer),
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<Arc<str>>, PromptDirectoryError> {
        let directory = self
            .inner
            .get_or_init(|| (self.initializer)().map(Arc::new))
            .map_err(shared_prompt_error)?;
        Ok(directory.get_shared(key))
    }
}

fn shared_prompt_error_detail(error: LazyInitError<io::Error>) -> Arc<io::Error> {
    match error {
        LazyInitError::Inner(error) => error,
        LazyInitError::ReentrantInitialization => Arc::new(io::Error::other(
            "reentrant prompt directory initialization",
        )),
    }
}

fn shared_prompt_error(error: LazyInitError<io::Error>) -> PromptDirectoryError {
    PromptDirectoryError::new(shared_prompt_error_detail(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, LazyLock, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;
    use text_assets_kit::TextResource;

    #[derive(Debug)]
    struct InnerPromptError;

    impl std::fmt::Display for InnerPromptError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("inner prompt error")
        }
    }

    impl std::error::Error for InnerPromptError {}

    fn failing_initializer() -> io::Result<TextDirectory> {
        Err(io::Error::other("prompt init failed"))
    }

    fn failing_initializer_with_source() -> io::Result<TextDirectory> {
        Err(io::Error::other(InnerPromptError))
    }

    fn failing_initializer_with_raw_os_error() -> io::Result<TextDirectory> {
        Err(io::Error::from_raw_os_error(2))
    }

    static REENTRANT_PROMPTS: LazyLock<LazyPromptDirectory> =
        LazyLock::new(|| LazyPromptDirectory::new(reentrant_initializer));

    fn reentrant_initializer() -> io::Result<TextDirectory> {
        REENTRANT_PROMPTS
            .get("default.md")
            .map_err(|error| io::Error::new(error.kind(), error))
            .map(|_| TextDirectory::default())
    }

    #[test]
    fn lazy_prompt_directory_reports_initializer_failure() {
        let catalog = LazyPromptDirectory::new(failing_initializer);

        let err = catalog.get("default.md").expect_err("init error");
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(err.to_string(), "prompt init failed");
    }

    #[test]
    fn lazy_prompt_directory_retries_after_initializer_failure() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let catalog = LazyPromptDirectory::new({
            let attempts = Arc::clone(&attempts);
            move || {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(io::Error::other("prompt init failed"))
                } else {
                    Ok(TextDirectory::default())
                }
            }
        });

        let err = catalog
            .get("default.md")
            .expect_err("first init should fail");
        assert_eq!(err.to_string(), "prompt init failed");

        assert_eq!(
            catalog.get("default.md").expect("second init should retry"),
            None
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lazy_prompt_directory_remains_uninitialized_while_retry_is_in_progress() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let catalog = Arc::new(LazyPromptDirectory::new({
            let attempts = Arc::clone(&attempts);
            let release_rx = Arc::clone(&release_rx);
            move || {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(io::Error::other("prompt init failed"))
                } else {
                    entered_tx.send(()).expect("signal retry entered");
                    release_rx
                        .lock()
                        .expect("lock release channel")
                        .recv()
                        .expect("release retry");
                    Ok(TextDirectory::default())
                }
            }
        }));

        let err = catalog
            .get("default.md")
            .expect_err("first init should fail");
        assert_eq!(err.to_string(), "prompt init failed");

        let retrying = Arc::clone(&catalog);
        let handle = thread::spawn(move || retrying.get("default.md"));
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("retry should start");

        release_tx.send(()).expect("release retry");
        let keys = handle
            .join()
            .expect("join retry thread")
            .expect("retry should succeed");
        assert_eq!(keys, None);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lazy_prompt_directory_new_accepts_captured_runtime_state() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("default.md"), "hello").expect("write prompt");
        let root = temp.path().to_path_buf();
        let catalog = LazyPromptDirectory::new(move || TextDirectory::load(&root));

        assert_eq!(
            catalog.get("default.md").expect("prompt value"),
            Some(Arc::<str>::from("hello"))
        );
    }

    #[test]
    fn bootstrap_prompt_directory_loads_manifest_resources() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid resource"));

        let directory = bootstrap_prompt_directory(temp.path(), &manifest).expect("bootstrap");
        assert_eq!(directory.get("default.md"), Some("hello"));
    }

    #[test]
    fn bootstrap_prompt_directory_waits_for_in_flight_bootstrap_load() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid resource"));
        let blocking_root = temp.path().to_path_buf();
        let blocking_manifest = manifest.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            bootstrap_prompt_directory_with_loader(&blocking_root, &blocking_manifest, |_, _| {
                entered_tx.send(()).expect("signal loader entered");
                release_rx.recv().expect("release loader");
                Ok(TextDirectory::default())
            })
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first loader should start");

        let waiting_root = temp.path().to_path_buf();
        let waiting_manifest = manifest.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            result_tx
                .send(
                    bootstrap_prompt_directory(&waiting_root, &waiting_manifest)
                        .map(|directory| directory.get("default.md").map(str::to_owned)),
                )
                .expect("publish second bootstrap result");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "second bootstrap should wait for the in-flight load to finish",
        );

        release_tx.send(()).expect("release first loader");
        handle
            .join()
            .expect("join first bootstrap thread")
            .expect("first bootstrap should succeed");
        waiter.join().expect("join second bootstrap thread");

        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("second bootstrap should complete")
                .expect("second bootstrap should succeed"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn bootstrap_prompt_directory_rebuilds_snapshot_from_current_disk_state() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid resource"));

        let first = bootstrap_prompt_directory(temp.path(), &manifest).expect("first bootstrap");
        assert_eq!(first.get("default.md"), Some("hello"));

        fs::write(temp.path().join("default.md"), "updated").expect("rewrite prompt");

        let second = bootstrap_prompt_directory(temp.path(), &manifest).expect("second bootstrap");
        assert_eq!(second.get("default.md"), Some("updated"));
        assert_eq!(first.get("default.md"), Some("hello"));
    }

    #[test]
    fn bootstrap_prompt_directory_ignores_unmanaged_root_files() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("notes.txt"), "ignore me").expect("write unrelated file");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid resource"));

        let directory = bootstrap_prompt_directory(temp.path(), &manifest).expect("bootstrap");
        assert_eq!(directory.get("default.md"), Some("hello"));
        assert_eq!(directory.get("notes.txt"), None);
        assert_eq!(
            directory
                .entries()
                .map(|(key, _)| key.to_string())
                .collect::<Vec<_>>(),
            vec!["default.md".to_string()]
        );
    }

    #[test]
    fn lazy_prompt_directory_preserves_error_source_chain() {
        let catalog = LazyPromptDirectory::new(failing_initializer_with_source);

        let error = catalog.get("default.md").expect_err("init error");
        let source = error.source().expect("wrapped source");
        assert_eq!(source.to_string(), "inner prompt error");
    }

    #[test]
    fn lazy_prompt_directory_rejects_reentrant_initialization() {
        let error = REENTRANT_PROMPTS
            .get("default.md")
            .expect_err("reentrant init should fail");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            error.to_string(),
            "reentrant prompt directory initialization"
        );
    }

    #[test]
    fn lazy_prompt_directory_waits_for_concurrent_initialization() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let catalog = Arc::new(LazyPromptDirectory::new({
            let release_rx = Arc::clone(&release_rx);
            move || {
                entered_tx.send(()).expect("signal initializer entered");
                release_rx
                    .lock()
                    .expect("lock release channel")
                    .recv()
                    .expect("release initializer");
                Ok(TextDirectory::default())
            }
        }));

        let initializing = Arc::clone(&catalog);
        let handle = thread::spawn(move || initializing.get("default.md"));

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initializer should start");

        let waiting = Arc::clone(&catalog);
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            result_tx
                .send(waiting.get("default.md"))
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
            None
        );
        assert_eq!(catalog.get("default.md").expect("catalog available"), None);
    }

    #[test]
    fn lazy_prompt_directory_default_api_preserves_raw_os_error() {
        let catalog = LazyPromptDirectory::new(failing_initializer_with_raw_os_error);

        let error = catalog.get("default.md").expect_err("init error");
        let io_error = error
            .source()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .expect("wrapped io::Error");
        assert_eq!(io_error.raw_os_error(), Some(2));
    }

    #[test]
    fn prompt_bootstrap_cleanup_error_preserves_both_failures() {
        let error = prompt_bootstrap_cleanup_error(
            io::Error::other("load failed"),
            io::Error::other("rollback failed"),
        );

        let cleanup = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<PromptBootstrapCleanupError>())
            .expect("wrapped cleanup error");
        assert_eq!(cleanup.load_error().to_string(), "load failed");
        assert_eq!(cleanup.rollback_error().to_string(), "rollback failed");
        assert_eq!(
            cleanup.source().expect("load source").to_string(),
            "load failed"
        );
        assert_eq!(
            cleanup.to_string(),
            "prompt directory load error: load failed; rollback failed: rollback failed"
        );
    }
}
