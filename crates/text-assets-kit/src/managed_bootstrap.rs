use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::Path;

use crate::resource_path::{
    materialize_resource_root_from_current_dir, materialize_resource_root_with_base,
};
use crate::{
    BootstrapReport, ResourceManifest, bootstrap_text_resources_with_report,
    bootstrap_text_resources_with_report_with_base, lock_bootstrap_transaction,
    rollback_created_resources,
};

#[derive(Debug)]
pub enum BootstrapLoadError<E> {
    Bootstrap(io::Error),
    Load(E),
    Rollback { load: E, rollback: io::Error },
}

impl<E> BootstrapLoadError<E> {
    #[must_use]
    pub fn map_load<F>(self, mut map: impl FnMut(E) -> F) -> BootstrapLoadError<F> {
        match self {
            Self::Bootstrap(error) => BootstrapLoadError::Bootstrap(error),
            Self::Load(error) => BootstrapLoadError::Load(map(error)),
            Self::Rollback { load, rollback } => BootstrapLoadError::Rollback {
                load: map(load),
                rollback,
            },
        }
    }
}

impl<E> Display for BootstrapLoadError<E>
where
    E: Display,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap(error) => write!(f, "bootstrap text resources: {error}"),
            Self::Load(error) => Display::fmt(error, f),
            Self::Rollback { load, rollback } => {
                write!(f, "load failed: {load}; rollback failed: {rollback}")
            }
        }
    }
}

impl<E> std::error::Error for BootstrapLoadError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bootstrap(error) => Some(error),
            Self::Load(error) => Some(error),
            Self::Rollback { load, .. } => Some(load),
        }
    }
}

/// Serializes same-root bootstrap/load attempts and rolls back resources
/// created by this attempt if the later `load` step fails.
///
/// Rollback is best-effort and scoped to files/directories created during the
/// current attempt. This helper does not provide a crash-safe or power-loss
/// recovery transaction.
pub fn bootstrap_text_resources_then_load<T, E, L>(
    root: &Path,
    manifest: &ResourceManifest,
    load: L,
) -> Result<T, BootstrapLoadError<E>>
where
    L: FnOnce(&Path, &[String]) -> Result<T, E>,
{
    let root =
        materialize_resource_root_from_current_dir(root).map_err(BootstrapLoadError::Bootstrap)?;
    bootstrap_text_resources_then_load_impl(root, manifest, load, |root, manifest| {
        bootstrap_text_resources_with_report(root, manifest)
    })
}

/// Equivalent to [`bootstrap_text_resources_then_load`] but resolves `root`
/// relative to an explicit absolute `base`.
pub fn bootstrap_text_resources_then_load_with_base<T, E, L>(
    base: &Path,
    root: &Path,
    manifest: &ResourceManifest,
    load: L,
) -> Result<T, BootstrapLoadError<E>>
where
    L: FnOnce(&Path, &[String]) -> Result<T, E>,
{
    let root =
        materialize_resource_root_with_base(base, root).map_err(BootstrapLoadError::Bootstrap)?;
    bootstrap_text_resources_then_load_impl(root.clone(), manifest, load, |_, manifest| {
        bootstrap_text_resources_with_report_with_base(base, &root, manifest)
    })
}

fn bootstrap_text_resources_then_load_impl<T, E, L, B>(
    root: std::path::PathBuf,
    manifest: &ResourceManifest,
    load: L,
    bootstrap: B,
) -> Result<T, BootstrapLoadError<E>>
where
    L: FnOnce(&Path, &[String]) -> Result<T, E>,
    B: FnOnce(&Path, &ResourceManifest) -> io::Result<BootstrapReport>,
{
    let resource_paths = manifest
        .resources()
        .iter()
        .map(|resource| resource.relative_path().to_owned())
        .collect::<Vec<_>>();
    let _bootstrap_transaction =
        lock_bootstrap_transaction(&root).map_err(BootstrapLoadError::Bootstrap)?;
    let report = bootstrap(&root, manifest).map_err(BootstrapLoadError::Bootstrap)?;

    match load(&root, &resource_paths) {
        Ok(value) => Ok(value),
        Err(load) => match rollback_created_resources(&report) {
            Ok(()) => Err(BootstrapLoadError::Load(load)),
            Err(rollback) => Err(BootstrapLoadError::Rollback { load, rollback }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::TextResource;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    struct CurrentDirGuard {
        original: std::path::PathBuf,
    }

    impl CurrentDirGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
            }
        }

        fn set(&self, path: &Path) {
            std::env::set_current_dir(path).expect("set cwd");
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.original).is_err() {
                std::env::set_current_dir("/").expect("restore cwd fallback");
            }
        }
    }

    fn managed_bootstrap_test_tempdir(test_name: &str) -> Option<TempDir> {
        let tempdir = tempfile::Builder::new()
            .prefix("of-bootstrap-")
            .rand_bytes(3)
            .tempdir_in(std::env::temp_dir())
            .unwrap_or_else(|err| panic!("temp dir: {err}"));
        let probe_root = tempdir.path().join("bootstrap-probe");
        let probe_manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid probe resource"));
        match crate::bootstrap_text_resources(&probe_root, &probe_manifest) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&probe_root);
                Some(tempdir)
            }
            Err(err) if err.kind() == io::ErrorKind::StorageFull => {
                eprintln!(
                    "skipping {test_name}: managed bootstrap temp root unavailable in this environment: {err}"
                );
                None
            }
            Err(err) => panic!("managed bootstrap probe: {err}"),
        }
    }

    fn skip_managed_bootstrap_storage_full<E>(
        test_name: &str,
        context: &str,
        err: &BootstrapLoadError<E>,
    ) -> bool {
        match err {
            BootstrapLoadError::Bootstrap(error) if error.kind() == io::ErrorKind::StorageFull => {
                eprintln!(
                    "skipping {test_name}: {context} unavailable in this environment: {error}"
                );
                true
            }
            _ => false,
        }
    }

    #[test]
    fn bootstrap_text_resources_then_load_with_base_uses_explicit_base_across_cwd_changes() {
        let cwd = CurrentDirGuard::new();
        let Some(temp) = managed_bootstrap_test_tempdir(
            "bootstrap_text_resources_then_load_with_base_uses_explicit_base_across_cwd_changes",
        ) else {
            return;
        };
        let workspace_a = temp.path().join("workspace_a");
        let workspace_b = temp.path().join("workspace_b");
        fs::create_dir_all(&workspace_a).expect("mkdir workspace_a");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace_b");
        cwd.set(&workspace_a);

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid resource"));

        let loaded_root = match bootstrap_text_resources_then_load_with_base(
            &workspace_a,
            Path::new("prompts"),
            &manifest,
            |root, resource_paths| {
                assert_eq!(resource_paths, ["default.md"]);
                Ok::<_, io::Error>(root.to_path_buf())
            },
        ) {
            Ok(loaded_root) => loaded_root,
            Err(err)
                if skip_managed_bootstrap_storage_full(
                    "bootstrap_text_resources_then_load_with_base_uses_explicit_base_across_cwd_changes",
                    "bootstrap with base",
                    &err,
                ) =>
            {
                return;
            }
            Err(err) => panic!("bootstrap with base: {err:?}"),
        };

        cwd.set(&workspace_b);
        assert_eq!(loaded_root, workspace_a.join("prompts"));
        assert_eq!(
            fs::read_to_string(workspace_a.join("prompts").join("default.md")).expect("read"),
            "hello"
        );
    }
}
