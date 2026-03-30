use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
fn unix_is_symlink_open_errno(code: i32) -> bool {
    match code {
        libc::ELOOP => true,
        #[cfg(any(
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "openbsd",
            target_os = "netbsd"
        ))]
        libc::EMLINK => true,
        _ => false,
    }
}

#[cfg(unix)]
pub fn is_symlink_or_reparse_open_error(err: &io::Error) -> bool {
    err.raw_os_error().is_some_and(unix_is_symlink_open_errno)
}

#[cfg(windows)]
fn windows_is_symlink_open_errno(code: i32) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_CANT_RESOLVE_FILENAME, ERROR_REPARSE_POINT_ENCOUNTERED, ERROR_STOPPED_ON_SYMLINK,
    };

    code == ERROR_STOPPED_ON_SYMLINK as i32
        || code == ERROR_REPARSE_POINT_ENCOUNTERED as i32
        || code == ERROR_CANT_RESOLVE_FILENAME as i32
}

#[cfg(windows)]
pub fn is_symlink_or_reparse_open_error(err: &io::Error) -> bool {
    err.raw_os_error()
        .is_some_and(windows_is_symlink_open_errno)
}

#[cfg(all(not(unix), not(windows)))]
pub fn is_symlink_or_reparse_open_error(_err: &io::Error) -> bool {
    false
}

#[cfg(unix)]
pub fn open_readonly_nofollow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path)
}

#[cfg(unix)]
pub fn open_writeonly_nofollow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path)
}

#[cfg(windows)]
pub fn open_readonly_nofollow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

#[cfg(windows)]
pub fn open_writeonly_nofollow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let mut options = OpenOptions::new();
    options
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

#[cfg(all(not(unix), not(windows)))]
pub fn open_readonly_nofollow(path: &Path) -> io::Result<File> {
    let _ = path;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "platform does not support atomic no-follow reads",
    ))
}

#[cfg(all(not(unix), not(windows)))]
pub fn open_writeonly_nofollow(path: &Path) -> io::Result<File> {
    let _ = path;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "platform does not support atomic no-follow writes",
    ))
}

fn ensure_regular_file(path: &Path, file: File) -> io::Result<(File, Metadata)> {
    let metadata = file.metadata()?;
    if metadata.is_file() {
        Ok((file, metadata))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a regular file: {}", path.display()),
        ))
    }
}

pub fn open_regular_readonly_nofollow(path: &Path) -> io::Result<(File, Metadata)> {
    let file = open_readonly_nofollow(path)?;
    ensure_regular_file(path, file)
}

pub fn open_regular_writeonly_nofollow(path: &Path) -> io::Result<(File, Metadata)> {
    let file = open_writeonly_nofollow(path)?;
    ensure_regular_file(path, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_symlink_open_is_classified_as_symlink_error() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("create tempdir");
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");

        std::fs::write(&target, b"x").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let err = open_readonly_nofollow(&link).expect_err("symlink open should fail");
        assert!(is_symlink_or_reparse_open_error(&err));
    }

    #[cfg(unix)]
    #[test]
    fn unix_eloop_errno_is_classified_as_symlink_error() {
        let err = io::Error::from_raw_os_error(libc::ELOOP);
        assert!(is_symlink_or_reparse_open_error(&err));
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    #[test]
    fn bsd_emlink_errno_is_classified_as_symlink_error() {
        let err = io::Error::from_raw_os_error(libc::EMLINK);
        assert!(is_symlink_or_reparse_open_error(&err));
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_symlink_errno_is_not_classified_as_symlink_error() {
        let err = io::Error::from_raw_os_error(libc::ENOENT);
        assert!(!is_symlink_or_reparse_open_error(&err));
    }

    #[cfg(unix)]
    #[test]
    fn unix_open_directory_does_not_imply_regular_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file = open_readonly_nofollow(dir.path()).expect("directory open should succeed");
        let meta = file
            .metadata()
            .expect("directory metadata should be available");
        assert!(
            !meta.is_file(),
            "open_readonly_nofollow must not be treated as a regular-file guarantee"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_open_regular_rejects_directory() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let err = open_regular_readonly_nofollow(dir.path())
            .expect_err("directory open should fail regular-file enforcement");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn unix_open_regular_accepts_regular_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file_path = dir.path().join("file.txt");
        std::fs::write(&file_path, b"ok").expect("write file");

        let (_file, metadata) =
            open_regular_readonly_nofollow(&file_path).expect("regular file open should succeed");
        assert!(metadata.is_file());
    }

    #[cfg(windows)]
    #[test]
    fn windows_only_expected_reparse_errors_are_classified_as_symlink_errors() {
        use windows_sys::Win32::Foundation::{
            ERROR_CANT_ACCESS_FILE, ERROR_CANT_RESOLVE_FILENAME, ERROR_INVALID_REPARSE_DATA,
            ERROR_NOT_A_REPARSE_POINT, ERROR_REPARSE_POINT_ENCOUNTERED, ERROR_STOPPED_ON_SYMLINK,
        };

        for code in [
            ERROR_STOPPED_ON_SYMLINK,
            ERROR_CANT_RESOLVE_FILENAME,
            ERROR_REPARSE_POINT_ENCOUNTERED,
        ] {
            assert!(
                windows_is_symlink_open_errno(code as i32),
                "helper must classify code {code} as symlink-related"
            );
            let err = io::Error::from_raw_os_error(code as i32);
            assert!(
                is_symlink_or_reparse_open_error(&err),
                "code {code} should be true"
            );
        }

        for code in [
            ERROR_CANT_ACCESS_FILE,
            ERROR_NOT_A_REPARSE_POINT,
            ERROR_INVALID_REPARSE_DATA,
        ] {
            assert!(
                !windows_is_symlink_open_errno(code as i32),
                "helper must reject non-symlink code {code}"
            );
            let err = io::Error::from_raw_os_error(code as i32);
            assert!(
                !is_symlink_or_reparse_open_error(&err),
                "code {code} should be false"
            );
        }
    }

    #[cfg(all(not(unix), not(windows)))]
    #[test]
    fn unsupported_platform_open_returns_unsupported() {
        let path = Path::new("dummy");
        let err = open_readonly_nofollow(path).expect_err("open must fail");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
