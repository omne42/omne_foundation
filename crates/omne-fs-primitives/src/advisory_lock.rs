use std::io;
use std::path::{Component, Path};

use fs2::FileExt;

use crate::{
    Dir, MissingRootPolicy, create_regular_file_at, open_ambient_root, open_regular_file_at,
};

#[derive(Debug)]
pub struct AdvisoryLockGuard {
    file: std::fs::File,
}

impl Drop for AdvisoryLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn lock_advisory_file_in_ambient_root(
    root: &Path,
    root_label: &str,
    file_name: &Path,
    file_label: &str,
) -> io::Result<AdvisoryLockGuard> {
    let file_name = validate_single_file_name(file_name, file_label)?;
    let directory = open_ambient_root(
        root,
        root_label,
        MissingRootPolicy::Create,
        |directory, component, _, error| {
            map_root_component_error(directory, component, error, root_label)
        },
    )?
    .map(|root| root.into_dir())
    .ok_or_else(|| io::Error::other(format!("{root_label} could not be created")))?;

    let file = match create_regular_file_at(&directory, file_name) {
        Ok(file) => file.into_std(),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            open_regular_file_at(&directory, file_name)
                .map(|file| file.into_std())
                .map_err(|error| map_regular_file_error(&directory, file_name, error, file_label))?
        }
        Err(error) => {
            return Err(map_regular_file_error(
                &directory, file_name, error, file_label,
            ));
        }
    };

    file.lock_exclusive()?;
    Ok(AdvisoryLockGuard { file })
}

fn validate_single_file_name<'a>(file_name: &'a Path, file_label: &str) -> io::Result<&'a Path> {
    let mut components = file_name.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) => Ok(Path::new(component)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{file_label} must be a single path component: {}",
                file_name.display()
            ),
        )),
    }
}

fn map_root_component_error(
    directory: &Dir,
    component: &Path,
    error: io::Error,
    root_label: &str,
) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{root_label} must stay within root without crossing symlinks"),
        ),
        Ok(metadata) if !metadata.is_dir() => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{root_label} component must be a directory: {}",
                component.display()
            ),
        ),
        _ => error,
    }
}

fn map_regular_file_error(
    directory: &Dir,
    component: &Path,
    error: io::Error,
    file_label: &str,
) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{file_label} must be a regular file without crossing symlinks"),
        ),
        Ok(metadata) if !metadata.is_file() => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{file_label} must be a regular file: {}",
                component.display()
            ),
        ),
        _ => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fs2::FileExt;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn lock_advisory_file_in_ambient_root_creates_lock_file() {
        let temp = TempDir::new().expect("temp dir");
        let lock_root = temp.path().join("locks");
        let _guard = lock_advisory_file_in_ambient_root(
            &lock_root,
            "lock namespace",
            Path::new("catalog.lock"),
            "lock file",
        )
        .expect("lock file");

        assert!(lock_root.join("catalog.lock").is_file());
    }

    #[test]
    fn lock_advisory_file_in_ambient_root_holds_exclusive_lock() {
        let temp = TempDir::new().expect("temp dir");
        let lock_root = temp.path().join("locks");
        let _guard = lock_advisory_file_in_ambient_root(
            &lock_root,
            "lock namespace",
            Path::new("catalog.lock"),
            "lock file",
        )
        .expect("lock file");

        let path = lock_root.join("catalog.lock");
        let competing = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open competing file handle");

        assert!(competing.try_lock_exclusive().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn lock_advisory_file_in_ambient_root_rejects_symlinked_root_component() {
        let temp = TempDir::new().expect("temp dir");
        let outside = temp.path().join("outside");
        let lock_root = temp.path().join("locks");
        fs::create_dir_all(&outside).expect("mkdir outside");
        symlink(&outside, &lock_root).expect("symlink root");

        let error = lock_advisory_file_in_ambient_root(
            &lock_root,
            "lock namespace",
            Path::new("catalog.lock"),
            "lock file",
        )
        .expect_err("symlinked root should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn lock_advisory_file_in_ambient_root_rejects_symlinked_lock_file() {
        let temp = TempDir::new().expect("temp dir");
        let lock_root = temp.path().join("locks");
        let outside = temp.path().join("outside.lock");
        fs::create_dir_all(&lock_root).expect("mkdir lock root");
        fs::write(&outside, "outside").expect("write outside");
        symlink(&outside, lock_root.join("catalog.lock")).expect("symlink lock file");

        let error = lock_advisory_file_in_ambient_root(
            &lock_root,
            "lock namespace",
            Path::new("catalog.lock"),
            "lock file",
        )
        .expect_err("symlinked file should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read_to_string(&outside).expect("outside contents"),
            "outside"
        );
    }
}
