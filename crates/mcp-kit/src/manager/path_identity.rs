use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::bail;

pub(crate) fn resolve_connection_cwd(cwd: &Path) -> anyhow::Result<PathBuf> {
    resolve_connection_cwd_with_base(None, cwd)
}

pub(crate) fn resolve_connection_cwd_with_base(
    base: Option<&Path>,
    cwd: &Path,
) -> anyhow::Result<PathBuf> {
    if cwd.is_absolute() {
        return stable_connection_cwd_identity(cwd);
    }

    let base = match base {
        Some(base) if base.is_absolute() => base,
        Some(base) => bail!("relative MCP cwd base must be absolute: {}", base.display()),
        None => bail!(
            "relative MCP cwd requires an explicit absolute base: {}",
            cwd.display()
        ),
    };
    resolve_relative_path_within_base(base, cwd, "relative MCP cwd")
}

pub(crate) fn stable_connection_cwd_identity(path: &Path) -> anyhow::Result<PathBuf> {
    stable_path_identity(path).map_err(Into::into)
}

pub(crate) fn resolve_relative_path_within_base(
    base: &Path,
    relative: &Path,
    label: &str,
) -> anyhow::Result<PathBuf> {
    debug_assert!(relative.is_relative(), "{label} must be relative");
    let resolved_base = stable_path_identity(base)?;
    let resolved_path = stable_path_identity(&base.join(relative))?;
    if !resolved_path.starts_with(&resolved_base) {
        bail!(
            "{label} must stay within root {}: {}",
            base.display(),
            relative.display()
        );
    }
    Ok(resolved_path)
}

pub(crate) fn stable_path_identity(path: &Path) -> io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut can_follow_existing_components = true;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                pop_without_crossing_root(&mut normalized);
            }
            Component::Normal(part) => {
                normalized.push(part);
                if !can_follow_existing_components {
                    continue;
                }

                match std::fs::symlink_metadata(&normalized) {
                    Ok(_) => {
                        normalized = std::fs::canonicalize(&normalized)?;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        can_follow_existing_components = false;
                    }
                    Err(err) => return Err(err),
                }
            }
        }
    }

    Ok(normalized)
}

fn pop_without_crossing_root(path: &mut PathBuf) {
    if path.file_name().is_some() {
        path.pop();
    }
}
