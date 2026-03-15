use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataRootScope {
    #[default]
    Auto,
    Workspace,
    Global,
}

#[derive(Debug, Clone)]
pub struct DataRootOptions {
    pub data_dir: Option<PathBuf>,
    pub scope: DataRootScope,
    pub dir_name: &'static str,
    pub env_var: &'static str,
    pub legacy_global_env_var: Option<&'static str>,
}

impl Default for DataRootOptions {
    fn default() -> Self {
        Self {
            data_dir: None,
            scope: DataRootScope::Auto,
            dir_name: ".omne_data",
            env_var: "OMNE_DATA_DIR",
            legacy_global_env_var: Some("OMNE_GLOBAL_ROOT"),
        }
    }
}

/// Resolves the runtime data root with the following precedence:
///
/// 1. `data_dir`
/// 2. `env_var`
/// 3. `legacy_global_env_var` when the scope is `Global` or `Auto`
/// 4. the default directory implied by `scope`
///
/// `Auto` first checks whether `<cwd>/<dir_name>` already exists. If it does,
/// the workspace-local directory wins; otherwise the resolver falls back to
/// `<home>/<dir_name>`.
pub fn resolve_data_root(options: &DataRootOptions) -> io::Result<PathBuf> {
    resolve_data_root_with(
        options,
        &|key| std::env::var_os(key),
        &std::env::current_dir,
        &Path::exists,
    )
}

fn resolve_data_root_with<F, C, E>(
    options: &DataRootOptions,
    env_lookup: &F,
    current_dir: &C,
    path_exists: &E,
) -> io::Result<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
    C: Fn() -> io::Result<PathBuf>,
    E: Fn(&Path) -> bool,
{
    if let Some(data_dir) = &options.data_dir {
        return Ok(data_dir.clone());
    }

    if let Some(data_dir) = lookup_env_path(env_lookup, options.env_var) {
        return Ok(data_dir);
    }

    if let Some(data_dir) = resolve_legacy_env_data_root(options, env_lookup) {
        return Ok(data_dir);
    }

    match options.scope {
        DataRootScope::Workspace => Ok(current_dir()?.join(options.dir_name)),
        DataRootScope::Global => Ok(resolve_home_dir_with(env_lookup)?.join(options.dir_name)),
        DataRootScope::Auto => {
            let workspace_root = current_dir()?.join(options.dir_name);
            if path_exists(&workspace_root) {
                Ok(workspace_root)
            } else {
                Ok(resolve_home_dir_with(env_lookup)?.join(options.dir_name))
            }
        }
    }
}

fn lookup_env_path<F>(env_lookup: &F, key: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    env_lookup(key).map(PathBuf::from)
}

fn resolve_legacy_env_data_root<F>(options: &DataRootOptions, env_lookup: &F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    if !matches!(options.scope, DataRootScope::Global | DataRootScope::Auto) {
        return None;
    }

    let env_var = options.legacy_global_env_var?;
    lookup_env_path(env_lookup, env_var)
}

pub fn ensure_data_root(options: &DataRootOptions) -> io::Result<PathBuf> {
    let root = resolve_data_root(options)?;
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

fn resolve_home_dir_with<F>(env_lookup: F) -> io::Result<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    env_lookup("HOME")
        .or_else(|| env_lookup("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot resolve data root: HOME or USERPROFILE is not set",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_data_dir_wins() {
        let root = resolve_data_root(&DataRootOptions {
            data_dir: Some(PathBuf::from("/tmp/omne")),
            ..DataRootOptions::default()
        })
        .expect("resolve root");
        assert_eq!(root, PathBuf::from("/tmp/omne"));
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
            &|_| false,
        )
        .expect("resolve root");
        assert_eq!(root, PathBuf::from("/tmp/omne_env"));
    }

    #[test]
    fn legacy_env_only_applies_to_global_and_auto_scope() {
        let workspace = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Workspace,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "OMNE_GLOBAL_ROOT" => Some(OsString::from("/legacy")),
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| false,
        )
        .expect("workspace root");
        assert_eq!(workspace, PathBuf::from("/workspace/.omne_data"));

        let global = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "OMNE_GLOBAL_ROOT" => Some(OsString::from("/legacy")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| false,
        )
        .expect("global root");
        assert_eq!(global, PathBuf::from("/legacy"));
    }

    #[test]
    fn global_scope_uses_home_fallbacks() {
        let from_home = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "HOME" => Some(OsString::from("/home/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| false,
        )
        .expect("global root from home");
        assert_eq!(from_home, PathBuf::from("/home/test/.omne_data"));

        let from_userprofile = resolve_data_root_with(
            &DataRootOptions {
                scope: DataRootScope::Global,
                ..DataRootOptions::default()
            },
            &|key| match key {
                "USERPROFILE" => Some(OsString::from("C:/Users/test")),
                _ => None,
            },
            &|| Ok(PathBuf::from("/workspace")),
            &|_| false,
        )
        .expect("global root from userprofile");
        assert_eq!(from_userprofile, PathBuf::from("C:/Users/test/.omne_data"));
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
            &|_| false,
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
            &|path| path == Path::new("/workspace/.omne_data"),
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
            &|_| false,
        )
        .expect("auto root");
        assert_eq!(root, PathBuf::from("/home/test/.omne_data"));
    }
}
