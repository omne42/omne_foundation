use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::OpenOptions;
pub use cap_std::fs::{Dir, File};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingRootPolicy {
    Error,
    ReturnNone,
    Create,
}

pub struct RootDir {
    path: PathBuf,
    dir: Dir,
}

impl RootDir {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_dir(self) -> Dir {
        self.dir
    }
}

fn materialize_root(root: &Path, label: &str) -> io::Result<PathBuf> {
    let root = normalize_root(root, label)?;
    validate_existing_ancestors(&root, label)
}

pub fn open_root<F>(
    root: &Path,
    label: &str,
    missing_root_policy: MissingRootPolicy,
    map_error: F,
) -> io::Result<Option<RootDir>>
where
    F: FnMut(&Dir, &Path, &Path, io::Error) -> io::Error,
{
    open_root_with_report(root, label, missing_root_policy, map_error)
        .map(|report| report.map(|(root, _)| root))
}

pub fn open_ambient_root<F>(
    root: &Path,
    label: &str,
    missing_root_policy: MissingRootPolicy,
    map_error: F,
) -> io::Result<Option<RootDir>>
where
    F: FnMut(&Dir, &Path, &Path, io::Error) -> io::Error,
{
    open_ambient_root_with_report(root, label, missing_root_policy, map_error)
        .map(|report| report.map(|(root, _)| root))
}

fn open_root_with_report<F>(
    root: &Path,
    label: &str,
    missing_root_policy: MissingRootPolicy,
    mut map_error: F,
) -> io::Result<Option<(RootDir, Vec<PathBuf>)>>
where
    F: FnMut(&Dir, &Path, &Path, io::Error) -> io::Error,
{
    let root = materialize_root(root, label)?;
    let (base, components) = split_root(&root, label)?;
    let mut current = Dir::open_ambient_dir(&base, ambient_authority())?;
    let mut current_path = base;
    let mut created_directories = Vec::new();

    for component in components {
        let component = Path::new(&component);
        match open_directory_component(&current, component) {
            Ok(next) => {
                current_path.push(component);
                current = next;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => match missing_root_policy {
                MissingRootPolicy::Create => {
                    let created = create_directory_component_if_missing(&current, component)
                        .map_err(|error| map_error(&current, component, &root, error))?;
                    current = open_directory_component(&current, component)
                        .map_err(|error| map_error(&current, component, &root, error))?;
                    current_path.push(component);
                    if created {
                        created_directories.push(current_path.clone());
                    }
                }
                MissingRootPolicy::ReturnNone => return Ok(None),
                MissingRootPolicy::Error => {
                    return Err(map_error(&current, component, &root, error));
                }
            },
            Err(error) => return Err(map_error(&current, component, &root, error)),
        }
    }

    Ok(Some((
        RootDir {
            path: root,
            dir: current,
        },
        created_directories,
    )))
}

fn open_ambient_root_with_report<F>(
    root: &Path,
    label: &str,
    missing_root_policy: MissingRootPolicy,
    mut map_error: F,
) -> io::Result<Option<(RootDir, Vec<PathBuf>)>>
where
    F: FnMut(&Dir, &Path, &Path, io::Error) -> io::Error,
{
    let root = normalize_root(root, label)?;
    let (base, components) = split_root(&root, label)?;
    let (ambient_base, managed_components) = split_ambient_existing_ancestor(&base, &components);
    let mut current = Dir::open_ambient_dir(&ambient_base, ambient_authority())?;
    let mut current_path = ambient_base;
    let mut created_directories = Vec::new();

    for component in managed_components {
        let component = Path::new(&component);
        match open_directory_component(&current, component) {
            Ok(next) => {
                current_path.push(component);
                current = next;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => match missing_root_policy {
                MissingRootPolicy::Create => {
                    let created = create_directory_component_if_missing(&current, component)
                        .map_err(|error| map_error(&current, component, &root, error))?;
                    current = open_directory_component(&current, component)
                        .map_err(|error| map_error(&current, component, &root, error))?;
                    current_path.push(component);
                    if created {
                        created_directories.push(current_path.clone());
                    }
                }
                MissingRootPolicy::ReturnNone => return Ok(None),
                MissingRootPolicy::Error => {
                    return Err(map_error(&current, component, &root, error));
                }
            },
            Err(error) => return Err(map_error(&current, component, &root, error)),
        }
    }

    Ok(Some((
        RootDir {
            path: root,
            dir: current,
        },
        created_directories,
    )))
}

pub fn open_directory_component(directory: &Dir, component: &Path) -> io::Result<Dir> {
    directory.open_dir_nofollow(component)
}

pub fn create_directory_component(directory: &Dir, component: &Path) -> io::Result<()> {
    create_directory_component_if_missing(directory, component).map(|_| ())
}

fn create_directory_component_if_missing(directory: &Dir, component: &Path) -> io::Result<bool> {
    match directory.create_dir(component) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = directory.symlink_metadata(component)?;
            if metadata.is_dir() {
                Ok(false)
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

pub fn open_regular_file_at(directory: &Dir, component: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.follow(FollowSymlinks::No);
    let file = match directory.open_with(component, &options) {
        Ok(file) => file,
        Err(error) => {
            if let Ok(metadata) = directory.symlink_metadata(component)
                && !metadata.is_file()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "target file must be a regular file",
                ));
            }
            return Err(error);
        }
    };
    ensure_regular_file(file)
}

pub fn create_regular_file_at(directory: &Dir, component: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.follow(FollowSymlinks::No);
    directory.open_with(component, &options)
}

fn ensure_regular_file(file: File) -> io::Result<File> {
    let metadata = file.metadata()?;
    if metadata.is_file() {
        return Ok(file);
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "target file must be a regular file",
    ))
}

fn normalize_root(root: &Path, label: &str) -> io::Result<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(invalid_root_path(label, root));
    }

    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()?.join(root)
    };

    let mut normalized = PathBuf::new();
    let mut saw_root = false;

    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                normalized.push(component.as_os_str());
                saw_root = true;
            }
            Component::Normal(part) => normalized.push(part),
            Component::CurDir | Component::ParentDir => {
                return Err(invalid_root_path(label, &absolute));
            }
        }
    }

    if !saw_root {
        return Err(invalid_root_path(label, &absolute));
    }

    Ok(normalized)
}

fn invalid_root_path(label: &str, path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{label} must be a normalized absolute path: {}",
            path.display()
        ),
    )
}

fn validate_existing_ancestors(root: &Path, label: &str) -> io::Result<PathBuf> {
    let mut validated = PathBuf::new();
    let mut components = root.components().peekable();

    while let Some(component) = components.next() {
        match component {
            Component::Prefix(_) | Component::RootDir => validated.push(component.as_os_str()),
            Component::Normal(part) => {
                validated.push(part);
                match std::fs::symlink_metadata(&validated) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "{label} must not traverse symlinks: {}",
                                validated.display()
                            ),
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        for remainder in components {
                            validated.push(remainder.as_os_str());
                        }
                        return Ok(validated);
                    }
                    Err(error) => return Err(error),
                }
            }
            Component::CurDir | Component::ParentDir => {
                return Err(invalid_root_path(label, root));
            }
        }
    }

    Ok(validated)
}

fn split_root(root: &Path, label: &str) -> io::Result<(PathBuf, Vec<OsString>)> {
    if !root.is_absolute() {
        return Err(invalid_root_path(label, root));
    }

    let mut base = PathBuf::new();
    let mut components = Vec::new();
    let mut saw_root = false;

    for component in root.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                base.push(component.as_os_str());
                saw_root = true;
            }
            Component::Normal(part) => components.push(part.to_os_string()),
            Component::CurDir | Component::ParentDir => return Err(invalid_root_path(label, root)),
        }
    }

    if !saw_root {
        return Err(invalid_root_path(label, root));
    }

    Ok((base, components))
}

fn split_ambient_existing_ancestor(
    base: &Path,
    components: &[OsString],
) -> (PathBuf, Vec<OsString>) {
    if components.is_empty() {
        return (base.to_path_buf(), Vec::new());
    }

    let mut ambient_base = base.to_path_buf();
    let mut managed_start = 0usize;

    while managed_start + 1 < components.len() {
        let candidate = ambient_base.join(&components[managed_start]);
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                ambient_base.push(&components[managed_start]);
                managed_start += 1;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(_) => break,
        }
    }

    (ambient_base, components[managed_start..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::TempDir;

    #[test]
    fn open_root_returns_none_when_missing_root_is_allowed() {
        let temp = TempDir::new().expect("temp dir");
        let base = temp.path().canonicalize().expect("canonicalize temp dir");
        let root = base.join("missing");

        let opened = open_root(
            &root,
            "test root",
            MissingRootPolicy::ReturnNone,
            |_, _, _, error| error,
        )
        .expect("open root");

        assert!(opened.is_none());
    }

    #[test]
    fn open_root_creates_missing_root_components() {
        let temp = TempDir::new().expect("temp dir");
        let base = temp.path().canonicalize().expect("canonicalize temp dir");
        let root = base.join("nested").join("root");

        let opened = open_root(
            &root,
            "test root",
            MissingRootPolicy::Create,
            |_, _, _, error| error,
        )
        .expect("create root")
        .expect("created root");

        assert_eq!(opened.path(), root.as_path());
        assert!(root.is_dir());
    }

    #[test]
    fn open_root_with_report_tracks_new_directories_only() {
        let temp = TempDir::new().expect("temp dir");
        let base = temp.path().canonicalize().expect("canonicalize temp dir");
        let existing = base.join("existing");
        fs::create_dir_all(&existing).expect("mkdir existing");
        let root = existing.join("nested").join("root");

        let (opened, created_directories) = open_root_with_report(
            &root,
            "test root",
            MissingRootPolicy::Create,
            |_, _, _, error| error,
        )
        .expect("create root")
        .expect("created root");

        assert_eq!(opened.path(), root.as_path());
        assert_eq!(
            created_directories,
            vec![existing.join("nested"), root.clone()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_ambient_root_allows_symlinked_existing_ancestor() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let real_parent = temp.path().join("real-parent");
        fs::create_dir_all(&real_parent).expect("mkdir real parent");
        symlink(&real_parent, temp.path().join("linked-parent")).expect("create symlink");
        let root = temp.path().join("linked-parent").join("locks");

        let opened = open_ambient_root(
            &root,
            "ambient root",
            MissingRootPolicy::Create,
            |_, _, _, error| error,
        )
        .expect("open ambient root")
        .expect("ambient root");

        assert_eq!(opened.path(), root.as_path());
        assert!(real_parent.join("locks").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn open_ambient_root_rejects_symlinked_target_component() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let parent = temp.path().join("parent");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&parent).expect("mkdir parent");
        fs::create_dir_all(&outside).expect("mkdir outside");
        symlink(&outside, parent.join("locks")).expect("create symlink target");

        let error = match open_ambient_root(
            &parent.join("locks"),
            "ambient root",
            MissingRootPolicy::Create,
            |directory, component, _, original| match directory.symlink_metadata(component) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    io::Error::new(io::ErrorKind::InvalidInput, "symlink target")
                }
                _ => original,
            },
        ) {
            Ok(_) => panic!("symlinked target should fail"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn materialize_root_preserves_missing_suffix_under_existing_ancestor() {
        let temp = TempDir::new().expect("temp dir");
        let base = temp.path().canonicalize().expect("canonicalize temp dir");
        let existing = base.join("existing");
        fs::create_dir_all(&existing).expect("mkdir existing");

        let materialized = materialize_root(&existing.join("missing").join("leaf"), "test root")
            .expect("materialize");

        assert_eq!(materialized, existing.join("missing").join("leaf"));
    }

    #[test]
    fn materialize_root_rejects_empty_path() {
        let error = materialize_root(Path::new(""), "test root").expect_err("empty path");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn materialize_root_rejects_parent_components() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("nested").join("..").join("root");

        let error = materialize_root(&root, "test root").expect_err("parent component");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn materialize_root_rejects_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let real = temp.path().join("real");
        fs::create_dir_all(real.join("nested")).expect("mkdir real nested");
        symlink(&real, temp.path().join("linked")).expect("create symlink");

        let error = materialize_root(&temp.path().join("linked").join("nested"), "test root")
            .expect_err("symlinked ancestor should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn open_regular_file_at_rejects_directories() {
        let temp = TempDir::new().expect("temp dir");
        let root = Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open temp dir");
        fs::create_dir(temp.path().join("nested")).expect("mkdir nested");

        let error =
            open_regular_file_at(&root, Path::new("nested")).expect_err("directory is not a file");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn create_directory_component_rejects_existing_file() {
        let temp = TempDir::new().expect("temp dir");
        let root = Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open temp dir");
        fs::write(temp.path().join("nested"), "not a dir").expect("write file");

        let error = create_directory_component(&root, Path::new("nested"))
            .expect_err("existing file should not count as a directory");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }
}
