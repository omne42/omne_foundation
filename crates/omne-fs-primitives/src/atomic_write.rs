use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AtomicWriteOptions {
    pub overwrite_existing: bool,
    pub create_parent_directories: bool,
    pub require_non_empty: bool,
    pub require_executable_on_unix: bool,
    pub unix_mode: Option<u32>,
}

impl Default for AtomicWriteOptions {
    fn default() -> Self {
        Self {
            overwrite_existing: true,
            create_parent_directories: true,
            require_non_empty: false,
            require_executable_on_unix: false,
            unix_mode: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AtomicDirectoryOptions {
    pub overwrite_existing: bool,
    pub create_parent_directories: bool,
}

impl Default for AtomicDirectoryOptions {
    fn default() -> Self {
        Self {
            overwrite_existing: true,
            create_parent_directories: true,
        }
    }
}

#[derive(Debug)]
pub struct StagedAtomicFile {
    destination: PathBuf,
    options: AtomicWriteOptions,
    staged: tempfile::NamedTempFile,
}

#[derive(Debug)]
pub struct StagedAtomicDirectory {
    destination: PathBuf,
    options: AtomicDirectoryOptions,
    staged: tempfile::TempDir,
}

#[derive(Debug)]
pub enum AtomicWriteError {
    IoPath {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    CommittedButUnsynced {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Validation(String),
}

#[derive(Debug)]
pub enum AtomicDirectoryError {
    IoPath {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    CommittedButUnsynced {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Validation(String),
}

impl AtomicWriteError {
    fn io_path(op: &'static str, path: &Path, source: io::Error) -> Self {
        Self::IoPath {
            op,
            path: path.to_path_buf(),
            source,
        }
    }

    fn committed_but_unsynced(op: &'static str, path: &Path, source: io::Error) -> Self {
        Self::CommittedButUnsynced {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl AtomicDirectoryError {
    fn io_path(op: &'static str, path: &Path, source: io::Error) -> Self {
        Self::IoPath {
            op,
            path: path.to_path_buf(),
            source,
        }
    }

    fn committed_but_unsynced(op: &'static str, path: &Path, source: io::Error) -> Self {
        Self::CommittedButUnsynced {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IoPath { op, path, source } => {
                write!(f, "io error during {op} ({}): {source}", path.display())
            }
            Self::CommittedButUnsynced { op, path, source } => write!(
                f,
                "filesystem update committed but parent sync failed during {op} ({}): {source}",
                path.display()
            ),
            Self::Validation(message) => write!(f, "{message}"),
        }
    }
}

impl fmt::Display for AtomicDirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IoPath { op, path, source } => {
                write!(f, "io error during {op} ({}): {source}", path.display())
            }
            Self::CommittedButUnsynced { op, path, source } => write!(
                f,
                "filesystem update committed but parent sync failed during {op} ({}): {source}",
                path.display()
            ),
            Self::Validation(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoPath { source, .. } | Self::CommittedButUnsynced { source, .. } => Some(source),
            Self::Validation(_) => None,
        }
    }
}

impl std::error::Error for AtomicDirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoPath { source, .. } | Self::CommittedButUnsynced { source, .. } => Some(source),
            Self::Validation(_) => None,
        }
    }
}

pub fn write_file_atomically(
    bytes: &[u8],
    destination: &Path,
    options: &AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let mut cursor = io::Cursor::new(bytes);
    write_file_atomically_from_reader(&mut cursor, destination, options)
}

pub fn write_file_atomically_from_reader<R>(
    reader: &mut R,
    destination: &Path,
    options: &AtomicWriteOptions,
) -> Result<(), AtomicWriteError>
where
    R: Read + ?Sized,
{
    let mut staged = stage_file_atomically(destination, options)?;
    io::copy(reader, staged.file_mut())
        .map_err(|err| AtomicWriteError::io_path("write", destination, err))?;
    staged.commit()
}

pub fn stage_file_atomically(
    destination: &Path,
    options: &AtomicWriteOptions,
) -> Result<StagedAtomicFile, AtomicWriteError> {
    stage_file_atomically_with_name(destination, options, None)
}

pub fn stage_file_atomically_with_name(
    destination: &Path,
    options: &AtomicWriteOptions,
    staged_file_name: Option<&str>,
) -> Result<StagedAtomicFile, AtomicWriteError> {
    if let Some(parent) = destination.parent()
        && options.create_parent_directories
    {
        fs::create_dir_all(parent)
            .map_err(|err| AtomicWriteError::io_path("create_dir_all", parent, err))?;
    }

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = staged_file_name
        .and_then(normalize_staged_file_name)
        .or_else(|| {
            destination
                .file_name()
                .and_then(|value| value.to_str())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "tool".to_string());
    let staged = tempfile::Builder::new()
        .prefix(&format!(".{file_name}.tmp-"))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|err| AtomicWriteError::io_path("create_temp", destination, err))?;

    Ok(StagedAtomicFile {
        destination: destination.to_path_buf(),
        options: options.clone(),
        staged,
    })
}

pub fn stage_directory_atomically(
    destination: &Path,
    options: &AtomicDirectoryOptions,
) -> Result<StagedAtomicDirectory, AtomicDirectoryError> {
    if let Some(parent) = destination.parent()
        && options.create_parent_directories
    {
        fs::create_dir_all(parent)
            .map_err(|err| AtomicDirectoryError::io_path("create_dir_all", parent, err))?;
    }

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| "tree".to_string());
    let staged = tempfile::Builder::new()
        .prefix(&format!(".{file_name}.tmpdir-"))
        .tempdir_in(parent)
        .map_err(|err| AtomicDirectoryError::io_path("create_tempdir", destination, err))?;

    Ok(StagedAtomicDirectory {
        destination: destination.to_path_buf(),
        options: options.clone(),
        staged,
    })
}

fn normalize_staged_file_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.replace(['/', '\\'], "_");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

impl StagedAtomicFile {
    pub fn file_mut(&mut self) -> &mut fs::File {
        self.staged.as_file_mut()
    }

    pub fn commit(mut self) -> Result<(), AtomicWriteError> {
        self.staged
            .as_file_mut()
            .flush()
            .map_err(|err| AtomicWriteError::io_path("flush", &self.destination, err))?;

        #[cfg(unix)]
        if let Some(mode) = self.options.unix_mode {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(mode);
            self.staged
                .as_file_mut()
                .set_permissions(perms)
                .map_err(|err| {
                    AtomicWriteError::io_path("set_permissions", &self.destination, err)
                })?;
        }

        self.staged
            .as_file_mut()
            .sync_all()
            .map_err(|err| AtomicWriteError::io_path("sync", &self.destination, err))?;

        let staged_path = self.staged.path().to_path_buf();
        validate_staged_file(&staged_path, &self.options)?;

        let persisted = self.staged.into_temp_path();
        commit_replace(
            persisted,
            &self.destination,
            self.options.overwrite_existing,
        )
    }
}

impl StagedAtomicDirectory {
    pub fn path(&self) -> &Path {
        self.staged.path()
    }

    pub fn commit(self) -> Result<(), AtomicDirectoryError> {
        validate_staged_directory(self.staged.path())?;
        commit_replace_directory(self.staged, &self.destination, &self.options)
    }
}

fn validate_staged_file(
    staged_path: &Path,
    options: &AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let metadata = fs::metadata(staged_path)
        .map_err(|err| AtomicWriteError::io_path("metadata", staged_path, err))?;
    if !metadata.is_file() {
        return Err(AtomicWriteError::Validation(format!(
            "staged file `{}` is not a regular file",
            staged_path.display()
        )));
    }
    if options.require_non_empty && metadata.len() == 0 {
        return Err(AtomicWriteError::Validation(format!(
            "staged file `{}` is empty",
            staged_path.display()
        )));
    }
    #[cfg(unix)]
    if options.require_executable_on_unix {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(AtomicWriteError::Validation(format!(
                "staged file `{}` is not executable",
                staged_path.display()
            )));
        }
    }
    Ok(())
}

fn validate_staged_directory(staged_path: &Path) -> Result<(), AtomicDirectoryError> {
    let metadata = fs::metadata(staged_path)
        .map_err(|err| AtomicDirectoryError::io_path("metadata", staged_path, err))?;
    if metadata.is_dir() {
        return Ok(());
    }
    Err(AtomicDirectoryError::Validation(format!(
        "staged directory `{}` is not a directory",
        staged_path.display()
    )))
}

fn commit_replace(
    staged_path: tempfile::TempPath,
    destination: &Path,
    overwrite_existing: bool,
) -> Result<(), AtomicWriteError> {
    if overwrite_existing {
        staged_path
            .persist(destination)
            .map_err(|err| AtomicWriteError::io_path("persist", destination, err.error))?;
    } else {
        staged_path.persist_noclobber(destination).map_err(|err| {
            AtomicWriteError::io_path("persist_noclobber", destination, err.error)
        })?;
    }
    sync_parent_directory(destination)
        .map_err(|err| AtomicWriteError::committed_but_unsynced("sync_parent", destination, err))
}

fn commit_replace_directory(
    staged_dir: tempfile::TempDir,
    destination: &Path,
    options: &AtomicDirectoryOptions,
) -> Result<(), AtomicDirectoryError> {
    let staged_path = staged_dir.keep();
    let mut backup_root = None;
    let mut backup_path = None;

    if options.overwrite_existing {
        let destination_metadata = match fs::symlink_metadata(destination) {
            Ok(metadata) => Some(metadata),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => {
                remove_path_if_exists(&staged_path);
                return Err(AtomicDirectoryError::io_path(
                    "symlink_metadata",
                    destination,
                    err,
                ));
            }
        };

        if let Some(metadata) = destination_metadata {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                remove_path_if_exists(&staged_path);
                return Err(AtomicDirectoryError::Validation(format!(
                    "directory destination `{}` must be an existing directory or absent",
                    destination.display()
                )));
            }
            let parent = destination.parent().unwrap_or_else(|| Path::new("."));
            let holder = tempfile::Builder::new()
                .prefix(".directory-backup-")
                .tempdir_in(parent)
                .map_err(|err| {
                    AtomicDirectoryError::io_path("create_backup_dir", destination, err)
                })?;
            let path = holder.path().join("previous");
            fs::rename(destination, &path).map_err(|err| {
                AtomicDirectoryError::io_path("rename_existing", destination, err)
            })?;
            backup_path = Some(path);
            backup_root = Some(holder);
        }
    }

    if let Err(err) = fs::rename(&staged_path, destination) {
        let restore_error = match backup_path.as_ref() {
            Some(path) => fs::rename(path, destination).err(),
            None => None,
        };
        remove_path_if_exists(&staged_path);
        let mut error = AtomicDirectoryError::io_path("rename_staged", destination, err);
        if let Some(restore_error) = restore_error {
            error = AtomicDirectoryError::Validation(format!(
                "{error}; restore existing directory `{}` failed: {restore_error}",
                destination.display()
            ));
        }
        return Err(error);
    }

    if let Some(holder) = backup_root {
        holder
            .close()
            .map_err(|err| AtomicDirectoryError::io_path("remove_backup_dir", destination, err))?;
    }

    sync_parent_directory(destination).map_err(|err| {
        AtomicDirectoryError::committed_but_unsynced("sync_parent", destination, err)
    })
}

fn remove_path_if_exists(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let result = if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    let _ = result;
}

#[cfg(all(not(windows), unix))]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    let parent_dir = fs::File::open(parent)?;
    parent_dir.sync_all()
}

#[cfg(not(all(not(windows), unix)))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use super::{
        AtomicDirectoryOptions, AtomicWriteError, AtomicWriteOptions, stage_directory_atomically,
        stage_file_atomically, stage_file_atomically_with_name, write_file_atomically,
    };

    #[test]
    fn atomic_write_creates_parent_directories_and_writes_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("nested/tool");

        let options = AtomicWriteOptions {
            create_parent_directories: true,
            require_non_empty: true,
            ..AtomicWriteOptions::default()
        };
        write_file_atomically(b"tool", &destination, &options).expect("write file");

        let content = std::fs::read(&destination).expect("read destination");
        assert_eq!(content, b"tool");
    }

    #[test]
    fn atomic_write_rejects_empty_file_when_required() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tool");

        let options = AtomicWriteOptions {
            require_non_empty: true,
            ..AtomicWriteOptions::default()
        };
        let err = write_file_atomically(b"", &destination, &options).expect_err("should fail");
        assert!(matches!(err, AtomicWriteError::Validation(_)));
    }

    #[test]
    fn atomic_write_replaces_existing_destination() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tool");
        std::fs::write(&destination, b"old").expect("seed file");

        let options = AtomicWriteOptions {
            overwrite_existing: true,
            require_non_empty: true,
            ..AtomicWriteOptions::default()
        };
        write_file_atomically(b"new", &destination, &options).expect("overwrite file");

        let content = std::fs::read(&destination).expect("read destination");
        assert_eq!(content, b"new");
    }

    #[test]
    fn staged_atomic_file_supports_read_before_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tool");

        let options = AtomicWriteOptions {
            require_non_empty: true,
            ..AtomicWriteOptions::default()
        };
        let mut staged = stage_file_atomically(&destination, &options).expect("stage file");
        staged.file_mut().write_all(b"tool").expect("write staged");
        staged
            .file_mut()
            .seek(SeekFrom::Start(0))
            .expect("rewind staged");
        let mut content = String::new();
        staged
            .file_mut()
            .read_to_string(&mut content)
            .expect("read staged");
        assert_eq!(content, "tool");
        staged.commit().expect("commit staged");

        let written = std::fs::read_to_string(&destination).expect("read destination");
        assert_eq!(written, "tool");
    }

    #[test]
    fn staged_atomic_file_uses_custom_temp_file_name_when_provided() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tool");
        let options = AtomicWriteOptions::default();
        let staged = stage_file_atomically_with_name(
            &destination,
            &options,
            Some("gh_9.9.9_linux_amd64.tar.gz"),
        )
        .expect("stage file");

        let name = staged
            .staged
            .path()
            .file_name()
            .and_then(|value| value.to_str())
            .expect("temp file name");
        assert!(name.starts_with(".gh_9.9.9_linux_amd64.tar.gz.tmp-"));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_sets_executable_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tool");

        let options = AtomicWriteOptions {
            unix_mode: Some(0o755),
            require_non_empty: true,
            require_executable_on_unix: true,
            ..AtomicWriteOptions::default()
        };
        write_file_atomically(b"#!/bin/sh\necho hi\n", &destination, &options).expect("write file");

        let mode = std::fs::metadata(&destination)
            .expect("metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }

    #[test]
    fn staged_atomic_directory_replaces_existing_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tree");
        std::fs::create_dir_all(&destination).expect("mkdir destination");
        std::fs::write(destination.join("old.txt"), b"old").expect("seed file");

        let staged = stage_directory_atomically(&destination, &AtomicDirectoryOptions::default())
            .expect("stage directory");
        std::fs::create_dir_all(staged.path().join("bin")).expect("mkdir staged");
        std::fs::write(staged.path().join("bin/tool"), b"new").expect("write staged file");

        staged.commit().expect("commit directory");

        assert!(!destination.join("old.txt").exists());
        assert_eq!(
            std::fs::read(destination.join("bin/tool")).expect("read staged file"),
            b"new"
        );
    }

    #[test]
    fn staged_atomic_directory_rejects_non_directory_destination() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("tree");
        std::fs::write(&destination, b"not a dir").expect("seed file");

        let staged = stage_directory_atomically(&destination, &AtomicDirectoryOptions::default())
            .expect("stage directory");

        let err = staged
            .commit()
            .expect_err("non-directory destination must fail");
        assert!(matches!(err, super::AtomicDirectoryError::Validation(_)));
    }
}
