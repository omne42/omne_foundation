use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use omne_fs_primitives::{
    Dir, MissingRootPolicy, RootDir, is_symlink_or_reparse_open_error, open_directory_component,
    open_regular_file_at, open_regular_readonly_nofollow, open_root,
    read_to_end_limited_with_capacity,
};

use crate::{ConfigFormat, Error, Result};

pub const DEFAULT_MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
pub const HARD_MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_INITIAL_CAPACITY: usize = 8 * 1024;
const MAX_INITIAL_CAPACITY: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigLoadOptions {
    max_bytes: u64,
    format: Option<ConfigFormat>,
    default_format: Option<ConfigFormat>,
}

impl ConfigLoadOptions {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_CONFIG_BYTES,
            format: None,
            default_format: None,
        }
    }

    #[must_use]
    pub const fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    #[must_use]
    pub const fn with_format(mut self, format: ConfigFormat) -> Self {
        self.format = Some(format);
        self
    }

    #[must_use]
    pub const fn with_default_format(mut self, default_format: ConfigFormat) -> Self {
        self.default_format = Some(default_format);
        self
    }

    #[must_use]
    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

impl Default for ConfigLoadOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDocument {
    path: PathBuf,
    format: ConfigFormat,
    contents: String,
}

impl ConfigDocument {
    #[must_use]
    pub fn new(path: PathBuf, format: ConfigFormat, contents: String) -> Self {
        Self {
            path,
            format,
            contents,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    #[must_use]
    pub const fn format(&self) -> ConfigFormat {
        self.format
    }

    #[must_use]
    pub fn contents(&self) -> &str {
        self.contents.as_str()
    }

    #[must_use]
    pub fn into_contents(self) -> String {
        self.contents
    }

    pub fn parse<T>(&self) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.format
            .parse_with_path(self.contents.as_str(), Some(self.path()))
    }

    pub fn parse_value(&self) -> Result<serde_json::Value> {
        self.format
            .parse_value_with_path(self.contents.as_str(), Some(self.path()))
    }
}

pub fn load_config_document(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
) -> Result<ConfigDocument> {
    match try_load_config_document(path.as_ref(), options)? {
        Some(document) => Ok(document),
        None => Err(Error::Io {
            action: "open",
            path: path.as_ref().to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "config file not found"),
        }),
    }
}

pub fn try_load_config_document(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
) -> Result<Option<ConfigDocument>> {
    let path = path.as_ref();
    validate_load_options(options)?;

    let format = resolve_format(path, options)?;
    let (mut file, metadata) = match open_regular_readonly_nofollow(path) {
        Ok(pair) => pair,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) if is_symlink_or_reparse_open_error(&err) => {
            return Err(Error::SymlinkPath {
                path: path.to_path_buf(),
            });
        }
        Err(err) => {
            return Err(Error::Io {
                action: "open",
                path: path.to_path_buf(),
                source: err,
            });
        }
    };
    let contents = read_document_contents(path, &mut file, metadata.len(), options.max_bytes)?;

    Ok(Some(ConfigDocument::new(
        path.to_path_buf(),
        format,
        contents,
    )))
}

pub fn find_config_document<I, P>(
    root: impl AsRef<Path>,
    candidates: I,
    options: ConfigLoadOptions,
) -> Result<Option<ConfigDocument>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let root = root.as_ref();
    validate_load_options(options)?;
    let Some(root_dir) = try_open_candidate_root(root)? else {
        return Ok(None);
    };
    let root_path = root_dir.path().to_path_buf();
    let root_dir = root_dir.into_dir();
    for candidate in candidates {
        if let Some(document) =
            try_load_rooted_candidate_document(&root_path, &root_dir, candidate.as_ref(), options)?
        {
            return Ok(Some(document));
        }
    }
    Ok(None)
}

pub fn canonicalize_path_in_root(
    root: impl AsRef<Path>,
    path: impl AsRef<Path>,
) -> Result<PathBuf> {
    let root = root.as_ref();
    let path = path.as_ref();
    let canonical_root = std::fs::canonicalize(root).map_err(|err| Error::Io {
        action: "canonicalize",
        path: root.to_path_buf(),
        source: err,
    })?;
    let canonical_path = std::fs::canonicalize(path).map_err(|err| Error::Io {
        action: "canonicalize",
        path: path.to_path_buf(),
        source: err,
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(Error::PathEscapesRoot {
            root: canonical_root,
            path: canonical_path,
        });
    }
    Ok(canonical_path)
}

fn validate_load_options(options: ConfigLoadOptions) -> Result<()> {
    if options.max_bytes == 0 {
        return Err(Error::InvalidOptions {
            message: "max_bytes must be greater than zero".to_string(),
        });
    }
    if options.max_bytes > HARD_MAX_CONFIG_BYTES {
        return Err(Error::InvalidOptions {
            message: format!("max_bytes exceeds hard limit of {HARD_MAX_CONFIG_BYTES} bytes"),
        });
    }
    Ok(())
}

fn resolve_format(path: &Path, options: ConfigLoadOptions) -> Result<ConfigFormat> {
    if let Some(format) = options.format {
        return Ok(format);
    }
    if let Some(format) = ConfigFormat::detect_opt(path)? {
        return Ok(format);
    }
    if let Some(default_format) = options.default_format {
        return Ok(default_format);
    }
    Err(Error::UnsupportedFormat {
        path: path.to_path_buf(),
        message: "path has no extension and no explicit/default format was provided".to_string(),
    })
}

fn initial_capacity(metadata_len: u64, max_bytes: u64) -> usize {
    usize::try_from(metadata_len.min(max_bytes))
        .ok()
        .map_or(DEFAULT_INITIAL_CAPACITY, |capacity| {
            capacity.min(MAX_INITIAL_CAPACITY)
        })
}

fn read_document_contents<R>(
    path: &Path,
    reader: &mut R,
    metadata_len: u64,
    max_bytes: u64,
) -> Result<String>
where
    R: Read,
{
    if metadata_len > max_bytes {
        return Err(Error::FileTooLarge {
            path: path.to_path_buf(),
            size_bytes: metadata_len,
            max_bytes,
        });
    }

    let max_bytes = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let (bytes, truncated) = read_to_end_limited_with_capacity(
        reader,
        max_bytes,
        initial_capacity(metadata_len, max_bytes as u64),
    )
    .map_err(|err| Error::Io {
        action: "read",
        path: path.to_path_buf(),
        source: err,
    })?;
    if truncated {
        return Err(Error::FileTooLarge {
            path: path.to_path_buf(),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            max_bytes: u64::try_from(max_bytes).unwrap_or(u64::MAX),
        });
    }

    String::from_utf8(bytes).map_err(|err| Error::InvalidUtf8 {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

fn try_open_candidate_root(root: &Path) -> Result<Option<RootDir>> {
    open_root(
        root,
        "config root",
        MissingRootPolicy::ReturnNone,
        |directory, component, _, error| map_candidate_directory_error(directory, component, error),
    )
    .map_err(|err| map_root_open_error(root, err))
}

fn try_load_rooted_candidate_document(
    root_path: &Path,
    root_dir: &Dir,
    candidate: &Path,
    options: ConfigLoadOptions,
) -> Result<Option<ConfigDocument>> {
    let (path, parent_components, leaf) = resolve_candidate_path(root_path, candidate)?;
    let format = resolve_format(&path, options)?;
    let mut directory = root_dir.try_clone().map_err(|err| Error::Io {
        action: "open",
        path: root_path.to_path_buf(),
        source: err,
    })?;

    for component in &parent_components {
        let component_path = Path::new(component);
        match open_directory_component(&directory, component_path) {
            Ok(next) => directory = next,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(map_candidate_component_open_error(
                    &directory,
                    component_path,
                    &path,
                    err,
                ));
            }
        }
    }

    let leaf_path = Path::new(&leaf);
    let mut file = match open_regular_file_at(&directory, leaf_path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(map_candidate_file_open_error(
                &directory, leaf_path, &path, err,
            ));
        }
    };
    let metadata = file.metadata().map_err(|err| Error::Io {
        action: "read",
        path: path.clone(),
        source: err,
    })?;
    let contents = read_document_contents(&path, &mut file, metadata.len(), options.max_bytes)?;

    Ok(Some(ConfigDocument::new(path, format, contents)))
}

fn resolve_candidate_path(
    root: &Path,
    candidate: &Path,
) -> Result<(PathBuf, Vec<OsString>, OsString)> {
    if candidate.is_absolute() {
        return Err(Error::InvalidOptions {
            message: format!(
                "config candidate path must be relative to root {}: {}",
                root.display(),
                candidate.display()
            ),
        });
    }

    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(Error::InvalidOptions {
            message: format!(
                "config candidate path must stay within root {}: {}",
                root.display(),
                candidate.display()
            ),
        });
    }

    let mut parent_components = Vec::new();
    let mut leaf = None;
    let mut components = candidate.components().peekable();

    while let Some(component) = components.next() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                if components.peek().is_some() {
                    parent_components.push(part.to_os_string());
                } else {
                    leaf = Some(part.to_os_string());
                }
            }
            Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::InvalidOptions {
                    message: format!(
                        "config candidate path must stay within root {}: {}",
                        root.display(),
                        candidate.display()
                    ),
                });
            }
            Component::RootDir => unreachable!("absolute paths are rejected above"),
        }
    }

    let Some(leaf) = leaf else {
        return Err(Error::InvalidOptions {
            message: format!(
                "config candidate path must point to a file under root {}: {}",
                root.display(),
                candidate.display()
            ),
        });
    };

    Ok((root.join(candidate), parent_components, leaf))
}

fn map_root_open_error(root: &Path, error: io::Error) -> Error {
    if root_open_error_is_symlink(&error) {
        return Error::SymlinkPath {
            path: root.to_path_buf(),
        };
    }

    Error::Io {
        action: "open",
        path: root.to_path_buf(),
        source: error,
    }
}

fn root_open_error_is_symlink(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidInput
        && error.to_string().contains("must not traverse symlinks")
}

fn map_candidate_component_open_error(
    directory: &Dir,
    component: &Path,
    path: &Path,
    error: io::Error,
) -> Error {
    if candidate_component_is_symlink(directory, component) {
        return Error::SymlinkPath {
            path: path.to_path_buf(),
        };
    }

    Error::Io {
        action: "open",
        path: path.to_path_buf(),
        source: error,
    }
}

fn map_candidate_file_open_error(
    directory: &Dir,
    component: &Path,
    path: &Path,
    error: io::Error,
) -> Error {
    if candidate_component_is_symlink(directory, component) {
        return Error::SymlinkPath {
            path: path.to_path_buf(),
        };
    }

    Error::Io {
        action: "open",
        path: path.to_path_buf(),
        source: error,
    }
}

fn map_candidate_directory_error(directory: &Dir, component: &Path, error: io::Error) -> io::Error {
    if candidate_component_is_symlink(directory, component) {
        return io::Error::new(
            io::ErrorKind::InvalidInput,
            "config candidate path must stay within root without crossing symlinks",
        );
    }

    match directory.symlink_metadata(component) {
        Ok(metadata) if !metadata.is_dir() => io::Error::new(
            error.kind(),
            format!(
                "config candidate path component must be a directory: {}",
                component.display()
            ),
        ),
        _ => error,
    }
}

fn candidate_component_is_symlink(directory: &Dir, component: &Path) -> bool {
    directory
        .symlink_metadata(component)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn canonicalize_rejects_paths_escaping_root_via_symlink() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("config.json");
        std::fs::write(&outside_file, "{}").expect("write outside file");

        let link = root.path().join("link");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");

        let err = canonicalize_path_in_root(root.path(), link.join("config.json"))
            .expect_err("escape must fail");
        assert!(err.to_string().contains("escapes root"));
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_symlink_files() {
        let dir = tempfile::tempdir().expect("dir");
        let target = dir.path().join("target.json");
        let link = dir.path().join("config.json");
        std::fs::write(&target, "{}").expect("target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let err =
            load_config_document(&link, ConfigLoadOptions::new()).expect_err("symlink must fail");
        assert!(err.to_string().contains("symlink"));
    }

    #[test]
    fn load_reads_typed_document() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "server:\n  enabled: true\n").expect("write");

        let document = load_config_document(&path, ConfigLoadOptions::new()).expect("load");
        assert_eq!(document.format(), ConfigFormat::Yaml);
        assert_eq!(
            document.parse_value().expect("value")["server"]["enabled"],
            serde_json::Value::Bool(true)
        );
    }

    #[test]
    fn load_supports_default_format_for_extensionless_paths() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config");
        std::fs::write(&path, "enabled = true\n").expect("write");

        let document = load_config_document(
            &path,
            ConfigLoadOptions::new().with_default_format(ConfigFormat::Toml),
        )
        .expect("load");
        assert_eq!(document.format(), ConfigFormat::Toml);
        assert_eq!(document.parse_value().expect("value")["enabled"], true);
    }

    #[test]
    fn load_rejects_oversized_files() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").expect("write");

        let err = load_config_document(&path, ConfigLoadOptions::new().with_max_bytes(1))
            .expect_err("too large");
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn find_returns_first_existing_candidate() {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::write(dir.path().join("b.toml"), "enabled = true\n").expect("write");

        let document =
            find_config_document(dir.path(), ["a.toml", "b.toml"], ConfigLoadOptions::new())
                .expect("find")
                .expect("document");
        assert_eq!(document.path(), dir.path().join("b.toml"));
    }

    #[test]
    fn find_rejects_parent_dir_candidates() {
        let dir = tempfile::tempdir().expect("dir");
        let err = find_config_document(dir.path(), ["../outside.toml"], ConfigLoadOptions::new())
            .expect_err("parent dir candidate must fail");
        assert!(err.to_string().contains("must stay within root"));
    }

    #[test]
    fn find_rejects_absolute_candidates() {
        let dir = tempfile::tempdir().expect("dir");
        let absolute = dir.path().join("config.toml");
        let err = find_config_document(dir.path(), [&absolute], ConfigLoadOptions::new())
            .expect_err("absolute candidate must fail");
        assert!(err.to_string().contains("must be relative to root"));
    }

    #[test]
    fn find_returns_none_when_root_is_missing() {
        let dir = tempfile::tempdir().expect("dir");
        let missing_root = dir.path().join("missing-root");

        let document =
            find_config_document(&missing_root, ["config.toml"], ConfigLoadOptions::new())
                .expect("missing root should not fail");
        assert!(document.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_rejects_candidate_paths_crossing_symlinked_directories() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("config.toml"), "enabled = true\n").expect("write");
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).expect("symlink");

        let err = find_config_document(
            root.path(),
            ["linked/config.toml"],
            ConfigLoadOptions::new(),
        )
        .expect_err("symlinked candidate path must fail");
        assert!(err.to_string().contains("symlink"));
    }
}
