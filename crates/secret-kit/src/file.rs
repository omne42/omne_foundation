use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use crate::{
    MAX_SECRET_FILE_BYTES, MAX_SECRET_FILE_SYMLINK_DEPTH, Result, SecretBytes, SecretError,
    SecretString, read_limited, secret_string_from_bytes,
};
use omne_fs_primitives::{
    Dir, MissingRootPolicy, open_directory_component, open_regular_file_at, open_root,
};

struct OpenedSecretFile {
    file: tokio::fs::File,
    path_text: String,
}

struct OpenedSecretFileBlocking {
    file: std::fs::File,
}

struct SecretFileScopeRoot {
    path: PathBuf,
    dir: Dir,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SecretFileEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

pub(crate) async fn read_secret_file(path: &Path) -> Result<SecretString> {
    let opened = open_secret_file(path).await?;
    read_opened_secret_file(opened).await
}

fn path_text(path: &Path) -> Cow<'_, str> {
    path.to_string_lossy()
}

async fn open_secret_file(path: &Path) -> Result<OpenedSecretFile> {
    let path_text = path_text(path).into_owned();
    let path_buf = path.to_path_buf();
    let blocking_path_text = path_text.clone();
    let opened = tokio::task::spawn_blocking(move || {
        open_secret_file_blocking(path_buf.as_path(), blocking_path_text.as_str())
    })
    .await
    .map_err(|err| {
        secret_file_read_failed(path_text.as_str(), std::io::Error::other(err.to_string()))
    })??;
    Ok(OpenedSecretFile {
        file: tokio::fs::File::from_std(opened.file),
        path_text,
    })
}

fn secret_file_read_failed(path_text: &str, error: std::io::Error) -> SecretError {
    secret_io_error!(
        "error_detail.secret.file_read_failed",
        error,
        "path" => path_text,
    )
}

fn secret_file_not_regular(path_text: &str, reason: &str) -> SecretError {
    secret_io_error!(
        "error_detail.secret.file_not_regular",
        std::io::Error::new(std::io::ErrorKind::InvalidInput, reason),
        "path" => path_text,
    )
}

fn secret_file_scope_error(path_text: &str, error: std::io::Error) -> SecretError {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        return secret_file_not_regular(
            path_text,
            "secret path must be a regular file or a symlink resolving within its parent directory",
        );
    }

    secret_file_read_failed(path_text, error)
}

fn open_secret_file_blocking(path: &Path, path_text: &str) -> Result<OpenedSecretFileBlocking> {
    let scope_root = open_secret_file_scope_root(path, path_text)?;
    let requested = requested_secret_file_component(path, path_text)?;
    let file = open_scoped_secret_file(&scope_root, requested, path_text, 0)?;
    Ok(OpenedSecretFileBlocking { file })
}

fn open_secret_file_scope_root(path: &Path, path_text: &str) -> Result<SecretFileScopeRoot> {
    open_root(
        secret_file_scope_root(path),
        "secret file scope",
        MissingRootPolicy::Error,
        |_, _, _, error| error,
    )
    .map_err(|error| secret_file_scope_error(path_text, error))?
    .map(|root| SecretFileScopeRoot {
        path: root.path().to_path_buf(),
        dir: root.into_dir(),
    })
    .ok_or_else(|| {
        secret_file_not_regular(
            path_text,
            "secret path must resolve within an existing parent directory",
        )
    })
}

fn requested_secret_file_component<'a>(path: &'a Path, path_text: &str) -> Result<&'a Path> {
    path.file_name()
        .map(Path::new)
        .filter(|component| !component.as_os_str().is_empty())
        .ok_or_else(|| {
            secret_file_not_regular(
                path_text,
                "secret path must resolve to a regular file within its parent directory",
            )
        })
}

fn open_scoped_secret_file(
    scope_root: &SecretFileScopeRoot,
    requested: &Path,
    path_text: &str,
    symlink_depth: usize,
) -> Result<std::fs::File> {
    if symlink_depth > MAX_SECRET_FILE_SYMLINK_DEPTH {
        return Err(secret_file_not_regular(
            path_text,
            "secret symlink chain exceeded the supported depth",
        ));
    }

    let requested = normalize_scoped_secret_relative_path(requested, path_text)?;
    if requested.as_os_str().is_empty() {
        return Err(secret_file_not_regular(
            path_text,
            "secret path must resolve to a regular file within its parent directory",
        ));
    }

    let mut current_dir = scope_root
        .dir
        .try_clone()
        .map_err(|error| secret_file_read_failed(path_text, error))?;
    let mut current_relative = PathBuf::new();
    let mut components = requested.components().peekable();

    while let Some(component) = components.next() {
        let component_path = Path::new(component.as_os_str());
        let is_last = components.peek().is_none();
        let kind = secret_file_entry_kind_at(&current_dir, component_path)
            .map_err(|error| secret_file_read_failed(path_text, error))?;

        match kind {
            SecretFileEntryKind::Directory if !is_last => {
                current_dir = open_directory_component(&current_dir, component_path)
                    .map_err(|error| secret_file_read_failed(path_text, error))?;
                current_relative.push(component.as_os_str());
            }
            SecretFileEntryKind::File if is_last => {
                let file = open_regular_file_at(&current_dir, component_path).map_err(|error| {
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
                    ) {
                        return secret_file_not_regular(
                            path_text,
                            "secret path must resolve to a regular file within its parent directory",
                        );
                    }

                    secret_file_read_failed(path_text, error)
                })?;
                return Ok(file.into_std());
            }
            SecretFileEntryKind::Symlink => {
                let target = current_dir
                    .read_link_contents(component_path)
                    .map_err(|error| secret_file_read_failed(path_text, error))?;
                let mut remainder = PathBuf::new();
                for rest in components {
                    remainder.push(rest.as_os_str());
                }
                let next = resolve_scoped_secret_link_target(
                    scope_root.path.as_path(),
                    &current_relative,
                    &target,
                    &remainder,
                    path_text,
                )?;
                return open_scoped_secret_file(
                    scope_root,
                    next.as_path(),
                    path_text,
                    symlink_depth + 1,
                );
            }
            SecretFileEntryKind::Directory
            | SecretFileEntryKind::File
            | SecretFileEntryKind::Other => {
                return Err(secret_file_not_regular(
                    path_text,
                    "secret path must resolve to a regular file within its parent directory",
                ));
            }
        }
    }

    Err(secret_file_not_regular(
        path_text,
        "secret path must resolve to a regular file within its parent directory",
    ))
}

fn resolve_scoped_secret_link_target(
    scope_root: &Path,
    current_relative: &Path,
    target: &Path,
    remainder: &Path,
    path_text: &str,
) -> Result<PathBuf> {
    if target.is_absolute() {
        return resolve_absolute_scoped_secret_link_target(
            scope_root, target, remainder, path_text,
        );
    }

    let mut combined = current_relative.to_path_buf();
    combined.push(target);
    combined.push(remainder);
    normalize_scoped_secret_relative_path(combined.as_path(), path_text)
}

fn resolve_absolute_scoped_secret_link_target(
    scope_root: &Path,
    target: &Path,
    remainder: &Path,
    path_text: &str,
) -> Result<PathBuf> {
    let mut combined = normalize_absolute_secret_path(target, path_text)?;
    combined.push(remainder);
    let combined = normalize_absolute_secret_path(combined.as_path(), path_text)?;
    let relative = combined.strip_prefix(scope_root).map_err(|_| {
        secret_file_not_regular(
            path_text,
            "secret symlink must resolve within its parent directory",
        )
    })?;
    normalize_scoped_secret_relative_path(relative, path_text)
}

fn secret_file_entry_kind_at(
    directory: &Dir,
    component: &Path,
) -> std::io::Result<SecretFileEntryKind> {
    let metadata = directory.symlink_metadata(component)?;
    let file_type = metadata.file_type();
    Ok(if file_type.is_symlink() {
        SecretFileEntryKind::Symlink
    } else if metadata.is_dir() {
        SecretFileEntryKind::Directory
    } else if metadata.is_file() {
        SecretFileEntryKind::File
    } else {
        SecretFileEntryKind::Other
    })
}

fn normalize_scoped_secret_relative_path(path: &Path, path_text: &str) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(secret_file_not_regular(
                        path_text,
                        "secret symlink must resolve within its parent directory",
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(secret_file_not_regular(
                    path_text,
                    "secret symlink must resolve within its parent directory",
                ));
            }
        }
    }
    Ok(normalized)
}

fn normalize_absolute_secret_path(path: &Path, path_text: &str) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut saw_root = false;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                normalized.push(prefix.as_os_str());
                saw_root = true;
            }
            Component::RootDir => {
                normalized.push(component.as_os_str());
                saw_root = true;
            }
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                let popped = normalized.pop();
                if !popped || normalized.as_os_str().is_empty() {
                    return Err(secret_file_not_regular(
                        path_text,
                        "secret symlink must resolve within its parent directory",
                    ));
                }
            }
        }
    }

    if !saw_root {
        return Err(secret_file_not_regular(
            path_text,
            "secret symlink must resolve within its parent directory",
        ));
    }

    Ok(normalized)
}

fn secret_file_scope_root(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

async fn read_opened_secret_file_bytes(mut opened: OpenedSecretFile) -> Result<SecretBytes> {
    let path_text = opened.path_text.as_str();
    let (out, truncated) = read_limited(&mut opened.file, MAX_SECRET_FILE_BYTES)
        .await
        .map_err(|err| secret_file_read_failed(path_text, err))?;
    if truncated {
        let source = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("secret file exceeded {MAX_SECRET_FILE_BYTES} bytes"),
        );
        return Err(secret_io_error!(
            "error_detail.secret.file_too_large",
            source,
            "path" => path_text,
            "max_bytes" => MAX_SECRET_FILE_BYTES.to_string()
        ));
    }
    Ok(out)
}

async fn read_opened_secret_file(opened: OpenedSecretFile) -> Result<SecretString> {
    let path_text = opened.path_text.clone();
    let bytes = read_opened_secret_file_bytes(opened).await?;
    secret_string_from_bytes(bytes, |utf8_error| {
        secret_file_read_failed(
            path_text.as_str(),
            std::io::Error::new(std::io::ErrorKind::InvalidData, utf8_error),
        )
    })
}
