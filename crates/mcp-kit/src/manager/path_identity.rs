use std::path::{Component, Path, PathBuf};

use anyhow::Context;

pub(super) fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

pub(super) fn canonicalize_existing_prefix(
    path: &Path,
    context: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let mut existing = path;
    let mut missing_components = Vec::new();

    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let Some(component) = existing.file_name() else {
                    return Ok(None);
                };
                missing_components.push(component.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Ok(None);
                };
                existing = parent;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "inspect existing path prefix for {context}: {}",
                        path.display()
                    )
                });
            }
        }
    }

    let mut resolved = std::fs::canonicalize(existing).with_context(|| {
        format!(
            "canonicalize existing path prefix for {context}: {}",
            existing.display()
        )
    })?;
    for component in missing_components.iter().rev() {
        resolved.push(component);
    }
    Ok(Some(resolved))
}
