use std::path::{Component, Path, PathBuf};

use anyhow::bail;

pub(crate) fn resolve_connection_cwd(cwd: &Path) -> anyhow::Result<PathBuf> {
    resolve_connection_cwd_with_base(None, cwd)
}

pub(crate) fn resolve_connection_cwd_with_base(
    base: Option<&Path>,
    cwd: &Path,
) -> anyhow::Result<PathBuf> {
    let resolved = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        let base = match base {
            Some(base) if base.is_absolute() => base.to_path_buf(),
            Some(base) => bail!("relative MCP cwd base must be absolute: {}", base.display()),
            None => bail!(
                "relative MCP cwd requires an explicit absolute base: {}",
                cwd.display()
            ),
        };
        base.join(cwd)
    };
    stable_connection_cwd_identity(&resolved)
}

pub(crate) fn stable_connection_cwd_identity(path: &Path) -> anyhow::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut can_follow_existing_components = true;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
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
                    Err(err) => return Err(err.into()),
                }
            }
        }
    }

    Ok(normalized)
}
