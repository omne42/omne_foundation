use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use omne_systems_fs_primitives::MissingRootPolicy;

use crate::resource_path::materialize_resource_root;
use crate::secure_fs::SecureRoot;

#[cfg(windows)]
const HOME_ENV_KEYS: &[&str] = &["HOME", "USERPROFILE"];
#[cfg(not(windows))]
const HOME_ENV_KEYS: &[&str] = &["HOME"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataRootScope {
    #[default]
    Auto,
    Workspace,
    Global,
}

#[derive(Debug, Clone)]
pub struct DataRootOptions {
    data_dir: Option<PathBuf>,
    scope: DataRootScope,
    dir_name: &'static str,
    env_var: &'static str,
}

impl DataRootOptions {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            data_dir: None,
            scope: DataRootScope::Auto,
            dir_name: ".omne_data",
            env_var: "OMNE_DATA_DIR",
        }
    }

    #[must_use]
    pub fn with_data_dir(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(data_dir.into());
        self
    }

    #[must_use]
    pub const fn with_scope(mut self, scope: DataRootScope) -> Self {
        self.scope = scope;
        self
    }
}

impl Default for DataRootOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceRootState {
    Missing,
    Directory,
    Invalid,
}

/// Resolves the runtime data root with the following precedence:
///
/// 1. `data_dir`
/// 2. `env_var`
/// 3. the default directory implied by `scope`
///
/// Explicit overrides must be absolute paths. Scope-specific defaults only
/// apply after the explicit overrides above.
/// Within the `Auto` default, `<cwd>/<dir_name>` wins when it already exists.
/// A missing workspace root falls back to `<home>/<dir_name>`, but an existing
/// invalid path is reported as an error instead of silently switching scopes.
pub fn resolve_data_root(options: &DataRootOptions) -> io::Result<PathBuf> {
    resolve_data_root_with(
        options,
        &|key| std::env::var_os(key),
        &std::env::current_dir,
        &workspace_root_state,
        &materialize_data_root,
    )
}

fn resolve_data_root_with<F, C, E, N>(
    options: &DataRootOptions,
    env_lookup: &F,
    current_dir: &C,
    workspace_root_state: &E,
    normalize_root: &N,
) -> io::Result<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
    C: Fn() -> io::Result<PathBuf>,
    E: Fn(&Path) -> io::Result<WorkspaceRootState>,
    N: Fn(&Path) -> io::Result<PathBuf>,
{
    if let Some(data_dir) = &options.data_dir {
        validate_absolute_data_root_path(data_dir, "data_dir")?;
        return normalize_root(data_dir);
    }

    if let Some(data_dir) = lookup_absolute_env_path(env_lookup, options.env_var)? {
        return normalize_root(&data_dir);
    }

    match options.scope {
        DataRootScope::Workspace => normalize_root(&current_dir()?.join(options.dir_name)),
        DataRootScope::Global => {
            normalize_root(&resolve_home_dir_with(env_lookup)?.join(options.dir_name))
        }
        DataRootScope::Auto => {
            let workspace_root = current_dir()?.join(options.dir_name);
            match workspace_root_state(&workspace_root)? {
                WorkspaceRootState::Missing => {}
                WorkspaceRootState::Directory => return normalize_root(&workspace_root),
                WorkspaceRootState::Invalid => {
                    return Err(invalid_workspace_root(&workspace_root));
                }
            }

            normalize_root(&resolve_home_dir_with(env_lookup)?.join(options.dir_name))
        }
    }
}

pub fn ensure_data_root(options: &DataRootOptions) -> io::Result<PathBuf> {
    let root = resolve_data_root(options)?;
    let _root = SecureRoot::open(&root, MissingRootPolicy::Create)?
        .ok_or_else(|| io::Error::other("resource data root could not be created"))?;
    Ok(root)
}

fn materialize_data_root(path: &Path) -> io::Result<PathBuf> {
    materialize_resource_root(path)
}

fn workspace_root_state(path: &Path) -> io::Result<WorkspaceRootState> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(WorkspaceRootState::Invalid),
        Ok(metadata) if metadata.is_dir() => Ok(WorkspaceRootState::Directory),
        Ok(_) => Ok(WorkspaceRootState::Invalid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(WorkspaceRootState::Missing),
        Err(error) => Err(error),
    }
}

fn invalid_workspace_root(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "workspace data root exists but is not a usable directory: {}",
            path.display()
        ),
    )
}

fn lookup_env_path<F>(env_lookup: &F, key: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    env_lookup(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn validate_absolute_data_root_path(path: &Path, label: &str) -> io::Result<()> {
    if path.is_absolute() {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("cannot resolve data root: {label} must be an absolute path"),
    ))
}

fn resolve_home_dir_with<F>(env_lookup: F) -> io::Result<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    let mut invalid_error = None;
    for key in HOME_ENV_KEYS {
        match lookup_absolute_env_path(&env_lookup, key) {
            Ok(Some(path)) => return Ok(path),
            Ok(None) => {}
            Err(error) if invalid_error.is_none() => invalid_error = Some(error),
            Err(_) => {}
        }
    }

    #[cfg(windows)]
    match lookup_windows_home_drive_path(&env_lookup) {
        Ok(Some(path)) => return Ok(path),
        Ok(None) => {}
        Err(error) if invalid_error.is_none() => invalid_error = Some(error),
        Err(_) => {}
    }

    if let Some(error) = invalid_error {
        return Err(error);
    }

    #[cfg(windows)]
    let missing_message =
        "cannot resolve data root: HOME, USERPROFILE, or HOMEDRIVE/HOMEPATH is not set";
    #[cfg(not(windows))]
    let missing_message = "cannot resolve data root: HOME is not set";

    Err(io::Error::new(io::ErrorKind::NotFound, missing_message))
}

fn lookup_absolute_env_path<F>(env_lookup: &F, key: &str) -> io::Result<Option<PathBuf>>
where
    F: Fn(&str) -> Option<OsString>,
{
    let Some(path) = lookup_env_path(env_lookup, key) else {
        return Ok(None);
    };

    if path.is_absolute() {
        return Ok(Some(path));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("cannot resolve data root: {key} must be an absolute path"),
    ))
}

#[cfg(windows)]
fn lookup_windows_home_drive_path<F>(env_lookup: &F) -> io::Result<Option<PathBuf>>
where
    F: Fn(&str) -> Option<OsString>,
{
    let Some(home_drive) = env_lookup("HOMEDRIVE").filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(home_path) = env_lookup("HOMEPATH").filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let mut combined = PathBuf::from(home_drive);
    combined.push(PathBuf::from(home_path));
    if combined.is_absolute() {
        return Ok(Some(combined));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "cannot resolve data root: HOMEDRIVE and HOMEPATH must form an absolute path",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn absolute_home_dir() -> &'static str {
        if cfg!(windows) {
            r"C:\Users\test"
        } else {
            "/home/test"
        }
    }

    #[cfg(windows)]
    fn absolute_userprofile_dir() -> &'static str {
        if cfg!(windows) {
            r"C:\Users\test"
        } else {
            "/Users/test"
        }
    }

    #[cfg(windows)]
    fn absolute_homedrive_parts() -> (&'static str, &'static str, PathBuf) {
        if cfg!(windows) {
            (r"C:", r"\Users\test", PathBuf::from(r"C:\Users\test"))
        } else {
            ("/", "home/test", PathBuf::from("/home/test"))
        }
    }

    fn passthrough_root(path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }

    #[test]
    fn explicit_data_dir_wins() {
        let root = resolve_data_root_with(
            &DataRootOptions {
                data_dir: Some(PathBuf::from("/tmp/omne")),
                ..DataRootOptions::default()
            },
            &|_| None,
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("resolve root");
        assert_eq!(root, PathBuf::from("/tmp/omne"));
    }

    #[test]
    fn explicit_relative_data_dir_is_rejected() {
        let error = resolve_data_root_with(
            &DataRootOptions {
                data_dir: Some(PathBuf::from("relative/omne")),
                ..DataRootOptions::default()
            },
            &|_| None,
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("relative explicit data dir should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("data_dir"));
    }

    #[test]
    fn env_var_wins_over_default() {
        let root = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "OMNE_DATA_DIR" => Some(OsString::from("/tmp/omne_env")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("resolve root");
        assert_eq!(root, PathBuf::from("/tmp/omne_env"));
    }

    #[test]
    fn relative_env_var_is_rejected() {
        let error = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "OMNE_DATA_DIR" => Some(OsString::from("relative/omne_env")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("relative env data dir should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("OMNE_DATA_DIR"));
    }

    #[test]
    fn empty_env_var_is_treated_as_unset() {
        let root = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "OMNE_DATA_DIR" => Some(OsString::new()),
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("resolve root");
        assert_eq!(root, PathBuf::from("/home/test/.omne_data"));
    }

    #[test]
    fn global_scope_uses_home_fallbacks() {
        let from_home = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::from(absolute_home_dir())),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("global root from home");
        assert_eq!(
            from_home,
            PathBuf::from(absolute_home_dir()).join(".omne_data")
        );
    }

    #[cfg(windows)]
    #[test]
    fn global_scope_uses_userprofile_fallback_on_windows() {
        let from_userprofile = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "USERPROFILE" => Some(OsString::from(absolute_userprofile_dir())),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("global root from userprofile");
        assert_eq!(
            from_userprofile,
            PathBuf::from(absolute_userprofile_dir()).join(".omne_data")
        );
    }

    #[cfg(windows)]
    #[test]
    fn empty_home_variables_are_ignored() {
        let root = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::new()),
                "USERPROFILE" => Some(OsString::from(absolute_userprofile_dir())),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("global root from userprofile");
        assert_eq!(
            root,
            PathBuf::from(absolute_userprofile_dir()).join(".omne_data")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn empty_home_variables_do_not_fall_back_to_windows_env_on_unix() {
        let error = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::new()),
                "USERPROFILE" => Some(OsString::from("/Users/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("unix fallback should ignore USERPROFILE");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("HOME"));
    }

    #[cfg(windows)]
    #[test]
    fn relative_home_uses_absolute_userprofile_fallback() {
        let root = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::from("relative/home")),
                "USERPROFILE" => Some(OsString::from(absolute_userprofile_dir())),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("global root from userprofile");
        assert_eq!(
            root,
            PathBuf::from(absolute_userprofile_dir()).join(".omne_data")
        );
    }

    #[cfg(windows)]
    #[test]
    fn relative_home_variables_are_rejected_without_absolute_fallback() {
        let error = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::from("relative/home")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("relative home should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("HOME"));
    }

    #[cfg(not(windows))]
    #[test]
    fn relative_home_variables_are_rejected_without_windows_fallbacks() {
        let error = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::from("relative/home")),
                "USERPROFILE" => Some(OsString::from("/Users/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("relative home should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("HOME"));
    }

    #[cfg(windows)]
    #[test]
    fn home_drive_and_home_path_form_global_fallback() {
        let (home_drive, home_path, home_root) = absolute_homedrive_parts();
        let root = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOMEDRIVE" => Some(OsString::from(home_drive)),
                "HOMEPATH" => Some(OsString::from(home_path)),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("global root from home drive and path");
        assert_eq!(root, home_root.join(".omne_data"));
    }

    #[cfg(not(windows))]
    #[test]
    fn home_drive_and_home_path_are_ignored_off_windows() {
        let error = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOMEDRIVE" => Some(OsString::from("/")),
                "HOMEPATH" => Some(OsString::from("home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("unix fallback should ignore HOMEDRIVE/HOMEPATH");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("HOME"));
    }

    #[test]
    fn global_scope_errors_without_home_variables() {
        let error = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|_| None,
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect_err("missing home should fail");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn auto_scope_prefers_existing_workspace_root() {
        let root = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|path| {
                Ok(if path == Path::new("/workspace/.omne_data") {
                    WorkspaceRootState::Directory
                } else {
                    WorkspaceRootState::Missing
                })
            },
            &passthrough_root,
        )
        .expect("auto root");
        assert_eq!(root, PathBuf::from("/workspace/.omne_data"));
    }

    #[test]
    fn auto_scope_falls_back_to_global_when_workspace_root_missing() {
        let root = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| Ok(WorkspaceRootState::Missing),
            &passthrough_root,
        )
        .expect("auto root");
        assert_eq!(root, PathBuf::from("/home/test/.omne_data"));
    }

    #[test]
    fn auto_scope_errors_when_workspace_root_is_not_directory() {
        let error = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|path| {
                Ok(if path == Path::new("/workspace/.omne_data") {
                    WorkspaceRootState::Invalid
                } else {
                    WorkspaceRootState::Missing
                })
            },
            &passthrough_root,
        )
        .expect_err("invalid workspace root should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("/workspace/.omne_data"));
    }

    #[test]
    fn auto_scope_errors_when_existing_workspace_root_cannot_be_materialized() {
        let error = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|path| {
                Ok(if path == Path::new("/workspace/.omne_data") {
                    WorkspaceRootState::Directory
                } else {
                    WorkspaceRootState::Missing
                })
            },
            &|path| {
                if path == Path::new("/workspace/.omne_data") {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "workspace root must not traverse symlinks",
                    ))
                } else {
                    Ok(path.to_path_buf())
                }
            },
        )
        .expect_err("invalid workspace root should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            error.to_string(),
            "workspace root must not traverse symlinks"
        );
    }

    #[cfg(unix)]
    #[test]
    fn auto_scope_errors_when_workspace_root_is_symlinked() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let home = temp.path().join("home");
        let linked_root = workspace.join(".omne_data");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        symlink(&outside, &linked_root).expect("symlink workspace root");

        let error = resolve_data_root_with(
            &DataRootOptions::default(),
            &|key| match key {
                "HOME" => Some(home.clone().into_os_string()),
                _ => None,
            },
            &|| Ok(workspace.clone()),
            &workspace_root_state,
            &materialize_data_root,
        )
        .expect_err("symlinked workspace root should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains(".omne_data"));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_data_root_rejects_symlinked_explicit_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = temp.path().join("outside");
        let linked_root = temp.path().join("linked_root");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        symlink(&outside, &linked_root).expect("symlink root");

        let error = ensure_data_root(&DataRootOptions {
            data_dir: Some(linked_root),
            ..DataRootOptions::default()
        })
        .expect_err("symlinked root should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
