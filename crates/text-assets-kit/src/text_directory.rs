use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
#[cfg(test)]
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use std::collections::BTreeSet;

use omne_fs_primitives::MissingRootPolicy;

use crate::resource_path::normalize_resource_path;
use crate::resource_path::{
    materialize_resource_root, materialize_resource_root_with_base, resource_identity_key,
};
use crate::secure_fs::SecureRoot;
use crate::secure_fs::validate_total_text_bytes;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextDirectory {
    entries: BTreeMap<String, Arc<str>>,
}

impl TextDirectory {
    /// Loads a text directory and treats a missing root as an error.
    pub fn load(root: &Path) -> io::Result<Self> {
        let root = materialize_resource_root(root)?;
        Self::load_materialized(&root)
    }

    /// Loads a text directory relative to an explicit absolute base path.
    pub fn load_with_base(base: &Path, root: &Path) -> io::Result<Self> {
        let root = materialize_resource_root_with_base(base, root)?;
        Self::load_materialized(&root)
    }

    pub fn load_resource_files(root: &Path, relative_paths: &[String]) -> io::Result<Self> {
        let root = materialize_resource_root(root)?;
        Self::load_resource_files_materialized(&root, relative_paths)
    }

    /// Loads selected text resource files relative to an explicit absolute
    /// base path.
    pub fn load_resource_files_with_base(
        base: &Path,
        root: &Path,
        relative_paths: &[String],
    ) -> io::Result<Self> {
        let root = materialize_resource_root_with_base(base, root)?;
        Self::load_resource_files_materialized(&root, relative_paths)
    }

    fn load_materialized(root: &Path) -> io::Result<Self> {
        let Some(root) = SecureRoot::open(root, MissingRootPolicy::ReturnNone)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("resource root does not exist: {}", root.display()),
            ));
        };
        let entries = build_entries(root.walk_text_files()?)?;
        Ok(Self { entries })
    }

    fn load_resource_files_materialized(
        root: &Path,
        relative_paths: &[String],
    ) -> io::Result<Self> {
        let root = SecureRoot::open(root, MissingRootPolicy::ReturnNone)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("resource root does not exist: {}", root.display()),
            )
        })?;

        let mut total_bytes = 0usize;
        let normalized_paths = normalize_unique_relative_paths(relative_paths)?;
        let mut loaded_entries = Vec::with_capacity(normalized_paths.len());
        for (key, relative_path) in normalized_paths {
            let contents = root.read_file_to_string(relative_path)?;
            total_bytes = total_bytes.saturating_add(contents.len());
            validate_total_text_bytes(total_bytes)?;
            loaded_entries.push((key, contents));
        }

        let entries = build_entries(loaded_entries)?;
        Ok(Self { entries })
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(Arc::as_ref)
    }

    #[must_use]
    pub fn get_shared(&self, key: &str) -> Option<Arc<str>> {
        self.entries.get(key).map(Arc::clone)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_ref()))
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
}

fn build_entries(entries: Vec<(String, String)>) -> io::Result<BTreeMap<String, Arc<str>>> {
    let mut directory = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for (key, contents) in entries {
        insert_entry(&mut directory, &mut identities, key, contents)?;
    }
    Ok(directory)
}

fn insert_entry(
    entries: &mut BTreeMap<String, Arc<str>>,
    identities: &mut BTreeMap<String, String>,
    key: String,
    contents: String,
) -> io::Result<()> {
    let identity = resource_identity_key(&key, false)?;
    if let Some(existing) = identities.get(&identity) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("duplicate text resource key {key:?} conflicts with existing key {existing:?}"),
        ));
    }

    match entries.entry(key) {
        Entry::Vacant(slot) => {
            identities.insert(identity, slot.key().clone());
            slot.insert(contents.into());
            Ok(())
        }
        Entry::Occupied(entry) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("duplicate text resource key {:?}", entry.key()),
        )),
    }
}

fn normalize_unique_relative_paths(relative_paths: &[String]) -> io::Result<Vec<(String, &str)>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(relative_paths.len());

    for relative_path in relative_paths {
        let key = normalize_resource_path(relative_path, false)?;
        let identity = resource_identity_key(&key, false)?;
        if !seen.insert(identity) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("duplicate text resource key {key:?}"),
            ));
        }
        normalized.push((key, relative_path.as_str()));
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secure_fs::{MAX_TEXT_DIRECTORY_TOTAL_BYTES, MAX_TEXT_RESOURCE_BYTES};
    use tempfile::TempDir;

    #[cfg(unix)]
    fn short_tempdir_for_unix_socket() -> TempDir {
        tempfile::Builder::new()
            .prefix("of-sock-")
            .rand_bytes(3)
            .tempdir_in("/var/tmp")
            .expect("short temp dir")
    }

    #[test]
    fn text_directory_loads_nested_files() {
        let temp = TempDir::new().expect("temp dir");
        let prompts_dir = temp.path().join("prompts");
        fs::create_dir_all(prompts_dir.join("nested")).expect("mkdir");
        fs::write(prompts_dir.join("default.md"), "hello").expect("write default");
        fs::write(prompts_dir.join("nested").join("system.md"), "world").expect("write nested");

        let directory = TextDirectory::load(&prompts_dir).expect("load");
        assert_eq!(directory.get("default.md"), Some("hello"));
        assert_eq!(directory.get("nested/system.md"), Some("world"));
        assert_eq!(
            directory.keys(),
            vec!["default.md".to_string(), "nested/system.md".to_string(),]
        );
    }

    #[test]
    fn text_directory_loads_deep_nested_files_without_recursion() {
        let temp = TempDir::new().expect("temp dir");
        let mut current = temp.path().to_path_buf();
        for _ in 0..128 {
            current = current.join("d");
            fs::create_dir_all(&current).expect("mkdir");
        }
        fs::write(current.join("leaf.txt"), "deep").expect("write deep file");

        let directory = TextDirectory::load(temp.path()).expect("load deep directory");
        let deep_key = format!("{}/leaf.txt", vec!["d"; 128].join("/"));
        assert_eq!(directory.get(&deep_key), Some("deep"));
    }

    #[test]
    fn text_directory_load_errors_when_root_is_missing() {
        let temp = TempDir::new().expect("temp dir");
        let missing = temp.path().join("missing");

        let error = TextDirectory::load(&missing).expect_err("missing root should fail");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn text_directory_load_with_base_uses_explicit_base_across_cwd_changes() {
        let temp = TempDir::new().expect("temp dir");
        let workspace_a = temp.path().join("workspace_a");
        let workspace_b = temp.path().join("workspace_b");
        let prompts_dir = workspace_a.join("prompts");
        fs::create_dir_all(prompts_dir.join("nested")).expect("mkdir prompts");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace_b");
        fs::write(prompts_dir.join("nested").join("system.md"), "hello").expect("write prompt");

        let original_cwd =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        std::env::set_current_dir(&workspace_b).expect("set cwd");
        let directory = TextDirectory::load_with_base(&workspace_a, Path::new("prompts"))
            .expect("load with base");
        if std::env::set_current_dir(&original_cwd).is_err() {
            std::env::set_current_dir("/").expect("restore cwd fallback");
        }

        assert_eq!(directory.get("nested/system.md"), Some("hello"));
    }

    #[test]
    fn text_directory_rejects_oversized_files_before_loading_them() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(
            temp.path().join("huge.txt"),
            vec![b'x'; MAX_TEXT_RESOURCE_BYTES + 1],
        )
        .expect("write oversized file");

        let err = TextDirectory::load(temp.path()).expect_err("oversized file should be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeds size limit"));
    }

    #[test]
    fn text_directory_rejects_directories_that_exceed_total_size_limit() {
        let temp = TempDir::new().expect("temp dir");
        let file_count = (MAX_TEXT_DIRECTORY_TOTAL_BYTES / MAX_TEXT_RESOURCE_BYTES) + 1;
        for index in 0..file_count {
            fs::write(
                temp.path().join(format!("file-{index}.txt")),
                vec![b'x'; MAX_TEXT_RESOURCE_BYTES],
            )
            .expect("write large file");
        }

        let err =
            TextDirectory::load(temp.path()).expect_err("oversized directory should be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("total size limit"));
    }

    #[test]
    fn build_entries_rejects_duplicate_keys() {
        let error = build_entries(vec![
            ("default.md".to_string(), "hello".to_string()),
            ("default.md".to_string(), "updated".to_string()),
        ])
        .expect_err("duplicate keys should fail");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn load_resource_files_rejects_directories_that_exceed_total_size_limit() {
        let temp = TempDir::new().expect("temp dir");
        let mut paths = Vec::new();
        for index in 0..9 {
            let path = format!("file-{index}.txt");
            fs::write(temp.path().join(&path), vec![b'x'; 950_000]).expect("write large file");
            paths.push(path);
        }

        let error = TextDirectory::load_resource_files(temp.path(), &paths)
            .expect_err("oversized explicit load should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("total size limit"));
    }

    #[test]
    fn load_resource_files_rejects_duplicate_paths_before_reading_contents() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("dup.txt"), vec![b'x'; 950_000]).expect("write large file");

        let error =
            TextDirectory::load_resource_files(temp.path(), &vec!["dup.txt".to_string(); 9])
                .expect_err("duplicate paths should fail before size accounting");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(error.to_string().contains("duplicate text resource key"));
    }

    #[test]
    fn load_resource_files_rejects_case_colliding_paths_before_reading_contents() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("Prompt.md"), "one").expect("write prompt");
        fs::write(temp.path().join("prompt.md"), "two").expect("write prompt");

        let error = TextDirectory::load_resource_files(
            temp.path(),
            &["Prompt.md".to_string(), "prompt.md".to_string()],
        )
        .expect_err("case-colliding paths should fail before loading");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(error.to_string().contains("duplicate text resource key"));
    }

    #[test]
    fn load_resource_files_reports_file_parent_components_as_directories() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("nested"), "not a directory").expect("write blocking file");

        let error =
            TextDirectory::load_resource_files(temp.path(), &["nested/file.md".to_string()])
                .expect_err("file parent should be rejected");

        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
        assert!(error.to_string().contains("must be a directory"));
    }

    #[test]
    fn text_directory_rejects_backslashes_in_file_names() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join(r"a\b.txt"), "bad").expect("write odd path");

        let err = TextDirectory::load(temp.path()).expect_err("load should reject backslash");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_duplicate_keys_after_component_joining() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("a")).expect("mkdir");
        fs::write(temp.path().join("a").join("b.txt"), "nested").expect("write nested");
        fs::write(temp.path().join(r"a\b.txt"), "flat").expect("write flat");

        let err =
            TextDirectory::load(temp.path()).expect_err("load should reject conflicting keys");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_case_colliding_file_names_for_portability() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("Prompt.md"), "first").expect("write prompt");
        fs::write(temp.path().join("prompt.md"), "second").expect("write prompt");

        let err = TextDirectory::load(temp.path()).expect_err("load should reject case collision");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("duplicate text resource key"));
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_colons_in_file_names() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("bad:name.txt"), "bad").expect("write odd path");

        let err = TextDirectory::load(temp.path()).expect_err("load should reject colon");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_windows_reserved_device_names() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("NUL.txt"), "bad").expect("write odd path");

        let err = TextDirectory::load(temp.path()).expect_err("load should reject reserved name");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Windows-reserved device name"));
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_symlinked_files() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, "secret").expect("write outside file");
        symlink(&outside, temp.path().join("linked.txt")).expect("create symlink");

        let err = TextDirectory::load(temp.path()).expect_err("load should reject symlink");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_symlinked_root_path() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        fs::write(outside.path().join("default.md"), "hello").expect("write prompt");
        let root = temp.path().join("linked_root");
        symlink(outside.path(), &root).expect("create root symlink");

        let err = TextDirectory::load(&root).expect_err("symlinked root should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn text_directory_rejects_socket_entries() {
        use std::os::unix::net::UnixListener;

        let temp = short_tempdir_for_unix_socket();
        let socket_path = temp.path().join("resource.sock");
        let _listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping text_directory_rejects_socket_entries: unix socket bind not permitted in this environment: {err}"
                );
                return;
            }
            Err(err) => panic!("bind socket: {err}"),
        };

        let err = TextDirectory::load(temp.path()).expect_err("socket entries should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("regular file or directory"));
    }
}
