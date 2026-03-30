use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub fn resolve_command_path(command: &str) -> Option<PathBuf> {
    resolve_command_path_os(OsStr::new(command))
}

pub fn resolve_command_path_os(command: &OsStr) -> Option<PathBuf> {
    resolve_command_path_os_with_path_var(command, std::env::var_os("PATH"))
}

pub(crate) fn resolve_command_path_os_with_path_var(
    command: &OsStr,
    path_var: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    for dir in std::env::split_paths(&path_var) {
        if let Some(path) = resolve_command_in_dir(command, &dir, is_spawnable_command_path) {
            return Some(path);
        }
    }
    None
}

pub fn resolve_command_path_or_standard_location(command: &str) -> Option<PathBuf> {
    resolve_command_path_or_standard_location_os(OsStr::new(command))
}

pub fn resolve_command_path_or_standard_location_os(command: &OsStr) -> Option<PathBuf> {
    resolve_command_path_os(command).or_else(|| {
        resolve_command_path_from_standard_locations(command, is_spawnable_command_path)
    })
}

pub(crate) fn resolve_available_command_path(command: &str) -> Option<PathBuf> {
    resolve_available_command_path_os(OsStr::new(command))
}

pub(crate) fn resolve_available_command_path_os(command: &OsStr) -> Option<PathBuf> {
    resolve_available_command_path_with_path_var(command, std::env::var_os("PATH"))
}

fn resolve_available_command_path_with_path_var(
    command: &OsStr,
    path_var: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    for dir in std::env::split_paths(&path_var) {
        if let Some(path) = resolve_command_in_dir(command, &dir, is_regular_command_path) {
            return Some(path);
        }
    }
    None
}

pub(crate) fn is_spawnable_command_path(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0)
    }

    #[cfg(windows)]
    {
        is_regular_command_path(path)
    }
}

pub(crate) fn is_regular_command_path(path: &Path) -> bool {
    path.is_file()
}

fn resolve_command_path_from_standard_locations<F>(command: &OsStr, predicate: F) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool + Copy,
{
    if command
        .to_string_lossy()
        .chars()
        .any(|ch| ch == '/' || ch == '\\')
    {
        return None;
    }

    #[cfg(not(windows))]
    let candidate_dirs = [
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/opt/homebrew/bin",
        "/opt/local/bin",
    ];
    #[cfg(windows)]
    let candidate_dirs: [&str; 0] = [];

    for dir in candidate_dirs {
        if let Some(path) = resolve_command_in_dir(command, Path::new(dir), predicate) {
            return Some(path);
        }
    }

    None
}

fn resolve_command_in_dir<F>(command: &OsStr, dir: &Path, predicate: F) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool + Copy,
{
    let candidate = dir.join(command);

    #[cfg(windows)]
    {
        let has_ext = Path::new(command).extension().is_some();
        if has_ext {
            return predicate(&candidate).then_some(candidate);
        }

        for ext in windows_path_extensions() {
            let ext_candidate = dir.join(format!("{}{}", command.to_string_lossy(), ext));
            if predicate(&ext_candidate) {
                return Some(ext_candidate);
            }
        }

        predicate(&candidate).then_some(candidate)
    }

    #[cfg(not(windows))]
    {
        predicate(&candidate).then_some(candidate)
    }
}

#[cfg(windows)]
fn windows_path_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::resolve_command_path;
    #[cfg(unix)]
    use super::{
        is_regular_command_path, is_spawnable_command_path, resolve_available_command_path,
        resolve_command_in_dir, resolve_command_path_or_standard_location,
    };
    #[cfg(unix)]
    use std::ffi::OsStr;

    #[test]
    fn missing_command_returns_none() {
        assert!(resolve_command_path("omne-process-primitives-missing-command").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn standard_locations_find_shell() {
        let resolved = resolve_command_path_or_standard_location("sh")
            .expect("resolve shell from PATH or standard locations");
        assert!(resolved.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_command_path_requires_executable_bit() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let executable = temp.path().join("tool");
        std::fs::write(&executable, "#!/bin/sh\n").expect("write executable");
        let mut executable_permissions = std::fs::metadata(&executable)
            .expect("stat executable")
            .permissions();
        executable_permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, executable_permissions).expect("chmod executable");

        let plain_file = temp.path().join("plain-tool");
        std::fs::write(&plain_file, "not executable").expect("write plain file");
        let mut plain_permissions = std::fs::metadata(&plain_file)
            .expect("stat plain file")
            .permissions();
        plain_permissions.set_mode(0o644);
        std::fs::set_permissions(&plain_file, plain_permissions).expect("chmod plain file");

        assert_eq!(
            resolve_command_in_dir(OsStr::new("tool"), temp.path(), is_spawnable_command_path),
            Some(executable.clone())
        );
        assert!(
            resolve_command_in_dir(
                OsStr::new("plain-tool"),
                temp.path(),
                is_spawnable_command_path
            )
            .is_none()
        );
        assert_eq!(
            resolve_command_in_dir(
                OsStr::new("plain-tool"),
                temp.path(),
                is_regular_command_path
            ),
            Some(plain_file.clone())
        );
        assert!(resolve_command_path("omne-process-primitives-missing-command").is_none());
        assert!(
            resolve_available_command_path("omne-process-primitives-missing-command").is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawnable_command_path_rejects_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let plain_file = temp.path().join("plain-tool");
        std::fs::write(&plain_file, "not executable").expect("write plain file");
        let mut permissions = std::fs::metadata(&plain_file)
            .expect("stat plain file")
            .permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&plain_file, permissions).expect("chmod plain file");

        assert!(is_regular_command_path(&plain_file));
        assert!(!is_spawnable_command_path(&plain_file));
    }
}
