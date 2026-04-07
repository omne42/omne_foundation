use std::io;
use std::path::{Path, PathBuf};

use omne_fs_primitives::{
    AdvisoryLockGuard, filesystem_is_case_sensitive, lock_advisory_file_in_ambient_root,
};
use std::collections::BTreeSet;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};

use crate::resource_path::materialize_resource_root_from_current_dir;

const BOOTSTRAP_LOCK_DIR_NAME: &str = ".text-assets-kit-bootstrap-locks";

struct BootstrapTransactionState {
    held_roots: Mutex<BTreeSet<BootstrapRootKey>>,
    ready: Condvar,
}

pub struct BootstrapTransactionGuard {
    root: BootstrapRootKey,
    lock_file: Option<AdvisoryLockGuard>,
}

static BOOTSTRAP_TRANSACTION_STATE: LazyLock<BootstrapTransactionState> =
    LazyLock::new(|| BootstrapTransactionState {
        held_roots: Mutex::new(BTreeSet::new()),
        ready: Condvar::new(),
    });

#[cfg(unix)]
type BootstrapRootKey = PathBuf;

#[cfg(not(unix))]
type BootstrapRootKey = String;

impl Drop for BootstrapTransactionGuard {
    fn drop(&mut self) {
        drop(self.lock_file.take());
        let mut held_roots = lock_unpoisoned(&BOOTSTRAP_TRANSACTION_STATE.held_roots);
        held_roots.remove(&self.root);
        BOOTSTRAP_TRANSACTION_STATE.ready.notify_all();
    }
}

/// Serializes bootstrap transactions within the current process.
///
/// This prevents rollback/load races between threads in one process, and
/// advisory file locking extends the exclusion across other cooperating local
/// processes by creating a hidden lock namespace under a stable same-filesystem
/// ancestor of the resource root instead of relying on an ambient temp
/// directory.
pub fn lock_bootstrap_transaction(root: &Path) -> io::Result<BootstrapTransactionGuard> {
    let root = materialize_resource_root_from_current_dir(root)?;
    let root_key = bootstrap_root_key(&root)?;
    let mut held_roots = lock_unpoisoned(&BOOTSTRAP_TRANSACTION_STATE.held_roots);
    while held_roots.contains(&root_key) {
        held_roots = wait_unpoisoned(&BOOTSTRAP_TRANSACTION_STATE.ready, held_roots);
    }
    held_roots.insert(root_key.clone());
    drop(held_roots);

    let lock_file = match open_bootstrap_lock_file(&root, &root_key) {
        Ok(lock_file) => lock_file,
        Err(error) => {
            let mut held_roots = lock_unpoisoned(&BOOTSTRAP_TRANSACTION_STATE.held_roots);
            held_roots.remove(&root_key);
            BOOTSTRAP_TRANSACTION_STATE.ready.notify_all();
            return Err(error);
        }
    };

    Ok(BootstrapTransactionGuard {
        root: root_key,
        lock_file: Some(lock_file),
    })
}

fn bootstrap_root_key(root: &Path) -> io::Result<BootstrapRootKey> {
    bootstrap_root_key_with(root, &symlink_metadata_path, &canonicalize_path)
}

fn bootstrap_root_key_with(
    root: &Path,
    symlink_metadata: &impl Fn(&Path) -> io::Result<std::fs::Metadata>,
    canonicalize: &impl Fn(&Path) -> io::Result<PathBuf>,
) -> io::Result<BootstrapRootKey> {
    let mut existing = root;
    let mut missing_components = Vec::new();

    while let Err(error) = symlink_metadata(existing) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "inspect bootstrap root prefix {}: {error}",
                    existing.display()
                ),
            ));
        }
        let Some(component) = existing.file_name() else {
            return Ok(normalized_bootstrap_root_key(root, true));
        };
        missing_components.push(component.to_os_string());
        let Some(parent) = existing.parent() else {
            return Ok(normalized_bootstrap_root_key(root, true));
        };
        existing = parent;
    }

    let case_sensitive = bootstrap_root_case_sensitive(existing);
    let mut canonical = canonicalize(existing).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "canonicalize bootstrap root prefix {}: {error}",
                existing.display()
            ),
        )
    })?;
    for component in missing_components.iter().rev() {
        canonical.push(component);
    }

    Ok(normalized_bootstrap_root_key(&canonical, case_sensitive))
}

fn symlink_metadata_path(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

fn canonicalize_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

fn open_bootstrap_lock_file(
    root: &Path,
    root_key: &BootstrapRootKey,
) -> io::Result<AdvisoryLockGuard> {
    open_bootstrap_lock_file_at(root_key, &bootstrap_lock_dir(root)?)
}

fn open_bootstrap_lock_file_at(
    root: &BootstrapRootKey,
    lock_dir: &Path,
) -> io::Result<AdvisoryLockGuard> {
    let lock_name = format!("{:016x}.lock", stable_bootstrap_lock_hash(root));
    lock_advisory_file_in_ambient_root(
        lock_dir,
        "bootstrap lock directory",
        Path::new(&lock_name),
        "bootstrap lock file",
    )
}

fn bootstrap_lock_dir(root: &Path) -> io::Result<PathBuf> {
    Ok(bootstrap_lock_anchor(root)?.join(BOOTSTRAP_LOCK_DIR_NAME))
}

fn bootstrap_lock_anchor(root: &Path) -> io::Result<PathBuf> {
    let mut existing = root;

    loop {
        match symlink_metadata_path(existing) {
            Ok(metadata) => {
                let anchor = if existing == root {
                    if metadata.is_dir() {
                        root
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("bootstrap root must be a directory: {}", root.display()),
                        ));
                    }
                } else if metadata.is_dir() {
                    existing
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "bootstrap lock anchor must be a directory: {}",
                            existing.display()
                        ),
                    ));
                };
                let canonical = canonicalize_path(anchor).map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "canonicalize bootstrap lock anchor {}: {error}",
                            anchor.display()
                        ),
                    )
                })?;
                #[cfg(unix)]
                {
                    return stabilize_bootstrap_lock_anchor_unix(&canonical);
                }
                #[cfg(not(unix))]
                {
                    return Ok(canonical);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                existing = existing.parent().unwrap_or(root);
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "inspect bootstrap lock anchor {}: {error}",
                        existing.display()
                    ),
                ));
            }
        }
    }
}

#[cfg(unix)]
fn stabilize_bootstrap_lock_anchor_unix(anchor: &Path) -> io::Result<PathBuf> {
    let device = symlink_metadata_path(anchor)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "inspect bootstrap lock anchor {}: {error}",
                    anchor.display()
                ),
            )
        })?
        .dev();

    let mut ancestors = vec![anchor.to_path_buf()];
    let mut current = anchor;
    while let Some(parent) = current.parent() {
        let metadata = symlink_metadata_path(parent).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "inspect bootstrap lock anchor ancestor {}: {error}",
                    parent.display()
                ),
            )
        })?;
        if metadata.dev() != device {
            break;
        }
        ancestors.push(parent.to_path_buf());
        current = parent;
    }

    ancestors.reverse();
    if ancestors.len() > 1
        && ancestors
            .first()
            .is_some_and(|path| path.parent().is_none())
    {
        ancestors.remove(0);
    }
    Ok(ancestors
        .into_iter()
        .next()
        .expect("canonical anchor chain must contain at least one path"))
}

fn stable_bootstrap_lock_hash(root: &BootstrapRootKey) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bootstrap_root_key_bytes(root) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn normalized_bootstrap_root_key(path: &Path, case_sensitive: bool) -> BootstrapRootKey {
    #[cfg(unix)]
    {
        if case_sensitive {
            path.to_path_buf()
        } else {
            normalize_case_insensitive_unix_root_key(path)
        }
    }

    #[cfg(windows)]
    {
        path.to_string_lossy().to_lowercase()
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(unix)]
fn bootstrap_root_case_sensitive(existing: &Path) -> bool {
    filesystem_is_case_sensitive(existing)
}

#[cfg(not(unix))]
fn bootstrap_root_case_sensitive(_existing: &Path) -> bool {
    true
}

#[cfg(unix)]
fn normalize_case_insensitive_unix_root_key(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(path) => PathBuf::from(path.to_lowercase()),
        None => path.to_path_buf(),
    }
}

#[cfg(unix)]
fn bootstrap_root_key_bytes(root: &BootstrapRootKey) -> &[u8] {
    root.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn bootstrap_root_key_bytes(root: &BootstrapRootKey) -> &[u8] {
    root.as_bytes()
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn wait_unpoisoned<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar
        .wait(guard)
        .unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::process::{Child, Command};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    const BOOTSTRAP_LOCK_HELPER_ENV: &str = "RUNTIME_ASSETS_KIT_BOOTSTRAP_LOCK_HELPER";
    const BOOTSTRAP_LOCK_ROOT_ENV: &str = "RUNTIME_ASSETS_KIT_BOOTSTRAP_LOCK_ROOT";
    const BOOTSTRAP_LOCK_HELD_ENV: &str = "RUNTIME_ASSETS_KIT_BOOTSTRAP_LOCK_HELD";
    const BOOTSTRAP_LOCK_RELEASE_ENV: &str = "RUNTIME_ASSETS_KIT_BOOTSTRAP_LOCK_RELEASE";
    const BOOTSTRAP_LOCK_TEST_FILTER: &str =
        "bootstrap_lock::tests::bootstrap_transaction_lock_blocks_other_processes";

    fn bootstrap_lock_test_tempdir(test_name: &str) -> Option<TempDir> {
        let tempdir = tempfile::Builder::new()
            .prefix("of-lock-")
            .rand_bytes(3)
            .tempdir_in(std::env::temp_dir())
            .unwrap_or_else(|err| panic!("temp dir: {err}"));
        let probe_root = tempdir.path().join("bootstrap-probe");
        match lock_bootstrap_transaction(&probe_root) {
            Ok(_guard) => Some(tempdir),
            Err(err) if err.kind() == io::ErrorKind::StorageFull => {
                eprintln!(
                    "skipping {test_name}: bootstrap lock temp root unavailable in this environment: {err}"
                );
                None
            }
            Err(err) => panic!("bootstrap lock probe: {err}"),
        }
    }

    fn helper_requested_skip(test_name: &str, held: &Path, child: &mut Child) -> bool {
        let contents = fs::read_to_string(held).unwrap_or_default();
        if let Some(reason) = contents.strip_prefix("skip:") {
            let status = child.wait().expect("wait for helper process after skip");
            assert!(status.success(), "helper process should exit cleanly");
            eprintln!("skipping {test_name}: {}", reason.trim());
            true
        } else {
            false
        }
    }

    fn wait_for_path(path: &Path, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }

        panic!("timed out waiting for path {}", path.display());
    }

    fn wait_for_reserved_root(root: &BootstrapRootKey, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(held_roots) = BOOTSTRAP_TRANSACTION_STATE.held_roots.try_lock()
                && held_roots.contains(root)
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }

        panic!("timed out waiting for reserved root {root:?}");
    }

    fn maybe_run_cross_process_lock_helper() -> bool {
        if std::env::var_os(BOOTSTRAP_LOCK_HELPER_ENV).is_none() {
            return false;
        }

        let root = PathBuf::from(
            std::env::var_os(BOOTSTRAP_LOCK_ROOT_ENV).expect("helper root must be set"),
        );
        let held = PathBuf::from(
            std::env::var_os(BOOTSTRAP_LOCK_HELD_ENV).expect("helper held path must be set"),
        );
        let release = PathBuf::from(
            std::env::var_os(BOOTSTRAP_LOCK_RELEASE_ENV).expect("helper release path must be set"),
        );

        let _guard = match lock_bootstrap_transaction(&root) {
            Ok(guard) => guard,
            Err(err) if err.kind() == io::ErrorKind::StorageFull => {
                fs::write(&held, format!("skip:{err}"))
                    .expect("helper should signal skipped lock setup");
                return true;
            }
            Err(err) => panic!("helper lock should succeed: {err}"),
        };
        fs::write(&held, "").expect("helper should signal held lock");
        wait_for_path(&release, Duration::from_secs(5));
        true
    }

    #[test]
    fn bootstrap_transaction_lock_waits_for_same_root() {
        let Some(temp) =
            bootstrap_lock_test_tempdir("bootstrap_transaction_lock_waits_for_same_root")
        else {
            return;
        };
        let root = temp.path().join("root");
        let blocking_root = root.clone();
        let waiting_root = root.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        let blocking = thread::spawn(move || {
            let _guard = lock_bootstrap_transaction(&blocking_root).expect("lock should succeed");
            entered_tx.send(()).expect("signal lock acquired");
            release_rx.recv().expect("release same-root lock");
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first lock should start");

        let waiting = thread::spawn(move || {
            let _guard = lock_bootstrap_transaction(&waiting_root).expect("lock should succeed");
            result_tx.send(()).expect("signal second lock acquired");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "same root should remain serialized",
        );

        release_tx.send(()).expect("release first lock");
        blocking.join().expect("join first lock holder");
        waiting.join().expect("join second lock holder");
        result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second lock should acquire after release");
    }

    #[test]
    fn bootstrap_transaction_lock_allows_distinct_roots() {
        let Some(temp) =
            bootstrap_lock_test_tempdir("bootstrap_transaction_lock_allows_distinct_roots")
        else {
            return;
        };
        let root_a = temp.path().join("root-a");
        let root_b = temp.path().join("root-b");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        let blocking = thread::spawn(move || {
            let _guard = lock_bootstrap_transaction(&root_a).expect("lock should succeed");
            entered_tx
                .send(())
                .expect("signal first root lock acquired");
            release_rx.recv().expect("release first root lock");
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first root lock should start");

        let waiting = thread::spawn(move || {
            let _guard = lock_bootstrap_transaction(&root_b).expect("lock should succeed");
            result_tx
                .send(())
                .expect("signal second root lock acquired");
        });

        result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("distinct roots should not block each other");

        release_tx.send(()).expect("release first root lock");
        blocking.join().expect("join first root lock holder");
        waiting.join().expect("join second root lock holder");
    }

    #[test]
    fn bootstrap_transaction_lock_blocks_other_processes() {
        if maybe_run_cross_process_lock_helper() {
            return;
        }

        let Some(temp) =
            bootstrap_lock_test_tempdir("bootstrap_transaction_lock_blocks_other_processes")
        else {
            return;
        };
        let root = temp.path().join("root");
        let held = temp.path().join("held");
        let release = temp.path().join("release");
        let current_exe = std::env::current_exe().expect("current test binary");

        let mut child = Command::new(current_exe)
            .arg(BOOTSTRAP_LOCK_TEST_FILTER)
            .arg("--exact")
            .env(BOOTSTRAP_LOCK_HELPER_ENV, "1")
            .env(BOOTSTRAP_LOCK_ROOT_ENV, &root)
            .env(BOOTSTRAP_LOCK_HELD_ENV, &held)
            .env(BOOTSTRAP_LOCK_RELEASE_ENV, &release)
            .spawn()
            .expect("spawn helper process");

        wait_for_path(&held, Duration::from_secs(5));
        if helper_requested_skip(
            "bootstrap_transaction_lock_blocks_other_processes",
            &held,
            &mut child,
        ) {
            return;
        }

        let waiting_root = root.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let waiting = thread::spawn(move || {
            let _guard =
                lock_bootstrap_transaction(&waiting_root).expect("cross-process wait should end");
            result_tx.send(()).expect("signal parent acquired lock");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "other process should keep the lock until it releases",
        );

        fs::write(&release, "").expect("release helper lock");
        waiting.join().expect("join waiting thread");
        result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("parent should acquire lock after helper exits");

        let status = child.wait().expect("wait for helper process");
        assert!(status.success(), "helper process should exit cleanly");
    }

    #[test]
    fn bootstrap_transaction_lock_waiting_on_other_process_does_not_block_distinct_roots() {
        if maybe_run_cross_process_lock_helper() {
            return;
        }

        let Some(temp) = bootstrap_lock_test_tempdir(
            "bootstrap_transaction_lock_waiting_on_other_process_does_not_block_distinct_roots",
        ) else {
            return;
        };
        let root_a = temp.path().join("root-a");
        let root_b = temp.path().join("root-b");
        let held = temp.path().join("held");
        let release = temp.path().join("release");
        let current_exe = std::env::current_exe().expect("current test binary");

        let mut child = Command::new(current_exe)
            .arg(BOOTSTRAP_LOCK_TEST_FILTER)
            .arg("--exact")
            .env(BOOTSTRAP_LOCK_HELPER_ENV, "1")
            .env(BOOTSTRAP_LOCK_ROOT_ENV, &root_a)
            .env(BOOTSTRAP_LOCK_HELD_ENV, &held)
            .env(BOOTSTRAP_LOCK_RELEASE_ENV, &release)
            .spawn()
            .expect("spawn helper process");

        wait_for_path(&held, Duration::from_secs(5));
        if helper_requested_skip(
            "bootstrap_transaction_lock_waiting_on_other_process_does_not_block_distinct_roots",
            &held,
            &mut child,
        ) {
            return;
        }

        let blocking_root = root_a.clone();
        let blocker = thread::spawn(move || {
            let _guard = lock_bootstrap_transaction(&blocking_root)
                .expect("lock should succeed after helper releases");
        });

        let blocked_root_key = bootstrap_root_key(&root_a);
        let blocked_root_key = blocked_root_key.expect("blocked root key");
        wait_for_reserved_root(&blocked_root_key, Duration::from_secs(1));

        let waiting_root = root_b.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let waiting = thread::spawn(move || {
            let _guard = lock_bootstrap_transaction(&waiting_root).expect("distinct root lock");
            result_tx.send(()).expect("signal distinct root acquired");
        });

        result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("distinct root should not wait on another root's external file lock");

        waiting.join().expect("join distinct root waiter");
        fs::write(&release, "").expect("release helper lock");
        blocker.join().expect("join blocked same-root waiter");

        let status = child.wait().expect("wait for helper process");
        assert!(status.success(), "helper process should exit cleanly");
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_transaction_lock_ignores_child_home_and_temp_env() {
        if maybe_run_cross_process_lock_helper() {
            return;
        }

        let Some(temp) = bootstrap_lock_test_tempdir(
            "bootstrap_transaction_lock_ignores_child_home_and_temp_env",
        ) else {
            return;
        };
        let root = temp.path().join("root");
        let held = temp.path().join("held");
        let release = temp.path().join("release");
        let child_home = temp.path().join("child-home");
        let child_tmp = temp.path().join("child-tmp");
        fs::create_dir_all(&child_home).expect("mkdir child home");
        fs::create_dir_all(&child_tmp).expect("mkdir child tmp");
        let current_exe = std::env::current_exe().expect("current test binary");

        let mut child = Command::new(current_exe)
            .arg(BOOTSTRAP_LOCK_TEST_FILTER)
            .arg("--exact")
            .env(BOOTSTRAP_LOCK_HELPER_ENV, "1")
            .env(BOOTSTRAP_LOCK_ROOT_ENV, &root)
            .env(BOOTSTRAP_LOCK_HELD_ENV, &held)
            .env(BOOTSTRAP_LOCK_RELEASE_ENV, &release)
            .env("HOME", &child_home)
            .env("TMPDIR", &child_tmp)
            .spawn()
            .expect("spawn helper process");

        wait_for_path(&held, Duration::from_secs(5));
        if helper_requested_skip(
            "bootstrap_transaction_lock_ignores_child_home_and_temp_env",
            &held,
            &mut child,
        ) {
            return;
        }

        let waiting_root = root.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let waiting = thread::spawn(move || {
            let _guard =
                lock_bootstrap_transaction(&waiting_root).expect("cross-process wait should end");
            result_tx.send(()).expect("signal parent acquired lock");
        });

        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "child environment differences must not bypass the shared bootstrap lock",
        );

        fs::write(&release, "").expect("release helper lock");
        waiting.join().expect("join waiting thread");
        result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("parent should acquire lock after helper exits");

        let status = child.wait().expect("wait for helper process");
        assert!(status.success(), "helper process should exit cleanly");
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_root_key_is_case_insensitive_on_windows() {
        let temp = TempDir::new().expect("temp dir");
        assert_eq!(
            bootstrap_root_key(&temp.path().join("Catalog")).expect("catalog key"),
            bootstrap_root_key(&temp.path().join("catalog")).expect("catalog key")
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalized_bootstrap_root_key_preserves_case_on_case_sensitive_unix() {
        assert_ne!(
            normalized_bootstrap_root_key(Path::new("/tmp/Catalog"), true),
            normalized_bootstrap_root_key(Path::new("/tmp/catalog"), true)
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalized_bootstrap_root_key_folds_case_on_case_insensitive_unix() {
        assert_eq!(
            normalized_bootstrap_root_key(Path::new("/tmp/Catalog"), false),
            normalized_bootstrap_root_key(Path::new("/tmp/catalog"), false)
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_root_key_preserves_non_utf8_bytes_on_unix() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("temp dir");
        let root_a = temp.path().join(OsString::from_vec(vec![b'r', 0xFF, b't']));
        let root_b = temp.path().join(OsString::from_vec(vec![b'r', 0xFE, b't']));
        let key_a = bootstrap_root_key(&root_a).expect("non-utf8 key");
        let key_b = bootstrap_root_key(&root_b).expect("non-utf8 key");

        assert_ne!(key_a, key_b);
        assert_ne!(
            stable_bootstrap_lock_hash(&key_a),
            stable_bootstrap_lock_hash(&key_b)
        );
    }

    #[test]
    fn bootstrap_root_key_rejects_non_not_found_metadata_errors() {
        let temp = TempDir::new().expect("temp dir");
        let file_path = temp.path().join("not-a-directory");
        fs::write(&file_path, "file").expect("write file");

        let error = bootstrap_root_key(&file_path.join("child"))
            .expect_err("non-directory prefix should not be treated as a missing path");
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains("inspect bootstrap root prefix"),
            "{error}"
        );
    }

    #[test]
    fn bootstrap_root_key_propagates_canonicalize_errors() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let prefix = temp.path().to_path_buf();
        let metadata = std::fs::symlink_metadata(&prefix).expect("prefix metadata");

        let error = bootstrap_root_key_with(
            &root,
            &|path| {
                if path == prefix.as_path() {
                    Ok(metadata.clone())
                } else {
                    Err(io::Error::from(io::ErrorKind::NotFound))
                }
            },
            &|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked")),
        )
        .expect_err("canonicalize failure should bubble up");

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            error
                .to_string()
                .contains("canonicalize bootstrap root prefix"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalize_case_insensitive_unix_root_key_folds_utf8_case() {
        assert_eq!(
            normalize_case_insensitive_unix_root_key(Path::new("/tmp/Catalog/SubDir")),
            PathBuf::from("/tmp/catalog/subdir")
        );
    }

    #[test]
    fn open_bootstrap_lock_file_keeps_a_stable_same_disk_namespace_as_root_materializes() {
        let Some(temp) = bootstrap_lock_test_tempdir(
            "open_bootstrap_lock_file_keeps_a_stable_same_disk_namespace_as_root_materializes",
        ) else {
            return;
        };
        let existing_prefix = temp.path().join("workspace");
        let root = existing_prefix.join("catalog").join("nested");
        fs::create_dir_all(&existing_prefix).expect("mkdir existing prefix");
        let root_key = bootstrap_root_key(&root).expect("root key");

        let lock_dir_before = bootstrap_lock_dir(&root).expect("lock dir before root exists");
        let _lock_file_before =
            open_bootstrap_lock_file(&root, &root_key).expect("open lock file before root exists");
        let lock_path_before = lock_dir_before.join(format!(
            "{:016x}.lock",
            stable_bootstrap_lock_hash(&root_key)
        ));
        assert!(lock_path_before.is_file());
        assert!(
            root.starts_with(
                lock_dir_before
                    .parent()
                    .expect("lock dir should have an anchor parent")
            )
        );
        drop(_lock_file_before);

        fs::create_dir_all(&root).expect("mkdir root");

        let lock_dir_after = bootstrap_lock_dir(&root).expect("lock dir after root exists");
        let _lock_file_after =
            open_bootstrap_lock_file(&root, &root_key).expect("open lock file after root exists");

        assert_eq!(
            lock_dir_before, lock_dir_after,
            "bootstrap lock namespace must stay stable after the resource root is created",
        );
        assert!(
            !root.join(BOOTSTRAP_LOCK_DIR_NAME).exists(),
            "lock namespace must stay outside the resource root",
        );
    }

    #[test]
    fn open_bootstrap_lock_file_creates_lock_in_target_directory() {
        let temp = TempDir::new().expect("temp dir");
        let lock_dir = temp.path().join("locks");
        let root = normalized_bootstrap_root_key(Path::new("catalog-root"), true);

        let _lock_file = open_bootstrap_lock_file_at(&root, &lock_dir).expect("open lock file");

        let lock_path = lock_dir.join(format!("{:016x}.lock", stable_bootstrap_lock_hash(&root)));
        assert!(lock_path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn open_bootstrap_lock_file_rejects_symlinked_lock_directory() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = temp.path().join("outside");
        let lock_dir = temp.path().join("locks");
        fs::create_dir_all(&outside).expect("mkdir outside");
        symlink(&outside, &lock_dir).expect("symlink lock dir");
        let root = normalized_bootstrap_root_key(Path::new("catalog-root"), true);

        let error = open_bootstrap_lock_file_at(&root, &lock_dir)
            .expect_err("symlinked lock directory should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn open_bootstrap_lock_file_rejects_symlinked_lock_file() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let lock_dir = temp.path().join("locks");
        let outside = temp.path().join("outside.lock");
        fs::create_dir_all(&lock_dir).expect("mkdir lock dir");
        fs::write(&outside, "outside").expect("write outside lock target");
        let root = normalized_bootstrap_root_key(Path::new("catalog-root"), true);
        let lock_path = lock_dir.join(format!("{:016x}.lock", stable_bootstrap_lock_hash(&root)));
        symlink(&outside, &lock_path).expect("symlink lock file");

        let error = open_bootstrap_lock_file_at(&root, &lock_dir)
            .expect_err("symlinked lock file should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read_to_string(&outside).expect("outside target should remain untouched"),
            "outside"
        );
    }
}
