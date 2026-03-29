use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::Path;

use crate::{
    ResourceManifest, bootstrap_text_resources_with_report, lock_bootstrap_transaction,
    materialize_resource_root, rollback_created_resources,
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

pub fn bootstrap_text_resources_then_load<T, E, L>(
    root: &Path,
    manifest: &ResourceManifest,
    load: L,
) -> Result<T, BootstrapLoadError<E>>
where
    L: FnOnce(&Path, &[String]) -> Result<T, E>,
{
    let root = materialize_resource_root(root).map_err(BootstrapLoadError::Bootstrap)?;
    let resource_paths = manifest
        .resources()
        .iter()
        .map(|resource| resource.relative_path().to_owned())
        .collect::<Vec<_>>();
    let _bootstrap_transaction =
        lock_bootstrap_transaction(&root).map_err(BootstrapLoadError::Bootstrap)?;
    let report = bootstrap_text_resources_with_report(&root, manifest)
        .map_err(BootstrapLoadError::Bootstrap)?;

    match load(&root, &resource_paths) {
        Ok(value) => Ok(value),
        Err(load) => match rollback_created_resources(&report) {
            Ok(()) => Err(BootstrapLoadError::Load(load)),
            Err(rollback) => Err(BootstrapLoadError::Rollback { load, rollback }),
        },
    }
}
