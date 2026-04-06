use std::path::{Component, Path, PathBuf};

#[cfg(test)]
pub(crate) fn resolve_connection_cwd(
    cwd: &Path,
    fallback_base: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    resolve_connection_cwd_with_base(None, cwd, fallback_base)
}

pub(crate) fn resolve_connection_cwd_with_base(
    base: Option<&Path>,
    cwd: &Path,
    fallback_base: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let resolved = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        let base = match base {
            Some(base) if base.is_absolute() => base.to_path_buf(),
            Some(base) => fallback_base
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "relative MCP cwd base requires a manager created from a valid current directory: {}",
                        base.display()
                    )
                })?
                .join(base),
            None => fallback_base
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "relative MCP cwd requires a manager created from a valid current directory"
                    )
                })?
                .to_path_buf(),
        };
        base.join(cwd)
    };
    Ok(stable_connection_cwd_identity(&resolved))
}

pub(crate) fn stable_connection_cwd_identity(path: &Path) -> PathBuf {
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
                        if let Ok(canonical) = std::fs::canonicalize(&normalized) {
                            normalized = canonical;
                        }
                    }
                    Err(_) => {
                        can_follow_existing_components = false;
                    }
                }
            }
        }
    }

    normalized
}
