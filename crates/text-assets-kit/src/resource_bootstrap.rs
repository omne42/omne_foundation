use std::io;
use std::path::{Path, PathBuf};

use omne_fs_primitives::{Dir, MissingRootPolicy};
use std::collections::BTreeSet;
use std::path::Component;

use crate::resource_path::{
    materialize_resource_root, normalize_resource_path, resource_identity_key,
};
use crate::secure_fs::{SecureRoot, WriteResult, validate_total_text_bytes};
use crate::text_resource::{ResourceManifest, validate_text_resource_contents};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapReport {
    root: PathBuf,
    rollback_base: PathBuf,
    created: Vec<PathBuf>,
    existing: Vec<PathBuf>,
    created_directories: Vec<PathBuf>,
    created_directory_paths: Vec<PathBuf>,
}

impl BootstrapReport {
    #[must_use]
    fn created_paths(&self) -> Vec<PathBuf> {
        self.created
            .iter()
            .map(|path| self.root.join(path))
            .collect()
    }

    #[must_use]
    fn created_directory_paths(&self) -> Vec<PathBuf> {
        self.created_directory_paths.clone()
    }

    fn new(root: PathBuf) -> Self {
        Self {
            rollback_base: root.clone(),
            root,
            created: Vec::new(),
            existing: Vec::new(),
            created_directories: Vec::new(),
            created_directory_paths: Vec::new(),
        }
    }

    fn set_base(&mut self, base: PathBuf) {
        self.rollback_base = base;
    }

    fn push_created_absolute(&mut self, path: PathBuf) -> io::Result<()> {
        self.created.push(self.relative_to_root(path)?);
        Ok(())
    }

    fn push_existing_absolute(&mut self, path: PathBuf) -> io::Result<()> {
        self.existing.push(self.relative_to_root(path)?);
        Ok(())
    }

    fn extend_created_directories_absolute(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if let Ok(relative) = self.relative_to_root(path.clone()) {
                self.created_directories.push(relative);
            }
            self.created_directory_paths.push(path);
        }
    }

    fn relative_to_root(&self, path: PathBuf) -> io::Result<PathBuf> {
        path.strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "bootstrap report path must stay under root: {}",
                        path.display()
                    ),
                )
            })
    }
}

/// Materializes a text resource manifest under `root` if files are absent.
///
/// This is the generic text-assets entry point for non-i18n, non-prompt
/// text resources such as default config files. Existing files are left in
/// place; newly created files and directories are rolled back automatically
/// if a later write in the same bootstrap attempt fails.
pub fn bootstrap_text_resources(
    root: impl AsRef<Path>,
    manifest: &ResourceManifest,
) -> io::Result<()> {
    bootstrap_text_resources_with_report(root.as_ref(), manifest).map(|_| ())
}

pub fn bootstrap_text_resources_with_report(
    root: &Path,
    manifest: &ResourceManifest,
) -> io::Result<BootstrapReport> {
    let root = materialize_resource_root(root)?;
    validate_manifest(manifest)?;
    let root_path = root.clone();
    let mut report = BootstrapReport::new(root_path.clone());
    let (root, created_root_directories) =
        SecureRoot::open_with_report(&root_path, MissingRootPolicy::Create)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "resource root could not be created",
            )
        })?;
    if let Some(rollback_base) = created_root_directories
        .first()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf)
    {
        report.set_base(rollback_base);
    }
    report.extend_created_directories_absolute(created_root_directories);

    for directory in directories_to_prepare(manifest) {
        match root.create_directory_all(directory) {
            Ok(created_directories) => report.extend_created_directories_absolute(
                created_directories
                    .into_iter()
                    .map(|directory| root_path.join(directory)),
            ),
            Err(error) => {
                return Err(finish_bootstrap_failure(&report, error));
            }
        }
    }

    for resource in manifest.resources() {
        let write_result =
            match root.write_file_if_absent(resource.relative_path(), resource.contents()) {
                Ok(write_result) => write_result,
                Err(error) => return Err(finish_bootstrap_failure(&report, error)),
            };

        match write_result {
            WriteResult::Created => {
                if let Err(error) =
                    report.push_created_absolute(root_path.join(resource.relative_path()))
                {
                    return Err(finish_bootstrap_failure(&report, error));
                }
            }
            WriteResult::ExistingFile => {
                if let Err(error) =
                    report.push_existing_absolute(root_path.join(resource.relative_path()))
                {
                    return Err(finish_bootstrap_failure(&report, error));
                }
            }
        }
    }

    Ok(report)
}

pub fn rollback_created_resources(report: &BootstrapReport) -> io::Result<()> {
    let mut first_error = None;
    let rollback_base = open_rollback_base(&report.rollback_base)?;
    for path in report.created_paths().iter().rev() {
        match remove_created_file(&rollback_base, &report.rollback_base, path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }

    for path in report.created_directory_paths().iter().rev() {
        match remove_created_directory(&rollback_base, &report.rollback_base, path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}

fn remove_created_file(base: &Dir, base_path: &Path, path: &Path) -> io::Result<()> {
    let path = rollback_relative_path(base_path, path)?;
    let (parent, leaf) = split_relative_leaf(path)?;
    let parent = open_rollback_directory(base, base_path, &parent)?;
    parent.remove_file(Path::new(leaf))
}

fn remove_created_directory(base: &Dir, base_path: &Path, path: &Path) -> io::Result<()> {
    let path = rollback_relative_path(base_path, path)?;
    let (parent, leaf) = split_relative_leaf(path)?;
    let parent = open_rollback_directory(base, base_path, &parent)?;
    parent.remove_dir(Path::new(leaf))
}

fn rollback_relative_path<'a>(base: &Path, path: &'a Path) -> io::Result<&'a Path> {
    path.strip_prefix(base).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "bootstrap report path must stay under rollback base: {}",
                path.display()
            ),
        )
    })
}

fn split_relative_leaf(path: &Path) -> io::Result<(Vec<&std::ffi::OsStr>, &std::ffi::OsStr)> {
    let mut components = relative_rollback_components(path)?;
    let leaf = components.pop().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("rollback path must not be empty: {}", path.display()),
        )
    })?;
    Ok((components, leaf))
}

fn relative_rollback_components(path: &Path) -> io::Result<Vec<&std::ffi::OsStr>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => components.push(part),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "rollback path must stay relative to the report base: {}",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(components)
}

fn open_rollback_base(path: &Path) -> io::Result<Dir> {
    omne_fs_primitives::open_root(
        path,
        "resource rollback base",
        MissingRootPolicy::Error,
        map_rollback_access_error,
    )?
    .map(|root| root.into_dir())
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "resource rollback base is missing"))
}

fn open_rollback_directory(
    base: &Dir,
    base_path: &Path,
    components: &[&std::ffi::OsStr],
) -> io::Result<Dir> {
    let mut directory = base.try_clone()?;
    let mut current_path = base_path.to_path_buf();
    for component in components {
        let component = Path::new(component);
        current_path.push(component);
        directory = omne_fs_primitives::open_directory_component(&directory, component).map_err(
            |error| map_rollback_access_error(&directory, component, &current_path, error),
        )?;
    }
    Ok(directory)
}

fn map_rollback_access_error(
    directory: &Dir,
    component: &Path,
    path: &Path,
    error: io::Error,
) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "resource rollback parent must not traverse symlinks: {}",
                path.display()
            ),
        ),
        Ok(_) => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "resource rollback path must be a directory: {}",
                path.display()
            ),
        ),
        Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => io::Error::new(
            io::ErrorKind::NotFound,
            format!("resource rollback path does not exist: {}", path.display()),
        ),
        _ => error,
    }
}

fn validate_manifest(manifest: &ResourceManifest) -> io::Result<()> {
    let mut resource_paths = std::collections::BTreeMap::<String, String>::new();
    let mut total_bytes = 0usize;
    for resource in manifest.resources() {
        let resource_path = normalize_resource_path(resource.relative_path(), false)?;
        let resource_identity = resource_identity_key(&resource_path, false)?;
        validate_text_resource_contents(&resource_path, resource.contents())?;
        total_bytes = total_bytes.saturating_add(resource.contents().len());
        validate_total_text_bytes(total_bytes)?;
        if resource_paths
            .insert(resource_identity.clone(), resource_path.clone())
            .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("duplicate resource path in manifest: {resource_path:?}"),
            ));
        }
    }

    for (resource_identity, resource_path) in &resource_paths {
        let mut ancestor = resource_identity.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if let Some(parent_path) = resource_paths.get(parent) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "resource paths conflict in manifest: {parent_path:?} and {resource_path:?}"
                    ),
                ));
            }
            ancestor = parent;
        }
    }

    Ok(())
}

fn finish_bootstrap_failure(report: &BootstrapReport, error: io::Error) -> io::Error {
    match rollback_created_resources(report) {
        Ok(()) => error,
        Err(rollback_error) => io::Error::new(
            rollback_error.kind(),
            format!("bootstrap text resources failed: {error}; rollback failed: {rollback_error}"),
        ),
    }
}

fn directories_to_prepare(manifest: &ResourceManifest) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    let mut directories = Vec::new();

    for resource in manifest.resources() {
        match resource_parent_directory(resource.relative_path()) {
            Some(parent_directory) if seen.insert(parent_directory) => {
                directories.push(parent_directory);
            }
            _ => {}
        }
    }

    directories
}

fn resource_parent_directory(relative_path: &str) -> Option<&str> {
    relative_path.rsplit_once('/').map(|(parent, _)| parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::secure_fs::{MAX_TEXT_DIRECTORY_TOTAL_BYTES, MAX_TEXT_RESOURCE_BYTES};
    use crate::text_resource::TextResource;
    use std::fs;

    use tempfile::TempDir;

    #[test]
    fn bootstrap_creates_directories_and_resources() {
        let temp = TempDir::new().expect("temp dir");

        let manifest = ResourceManifest::new()
            .with_resource(
                TextResource::new("i18n/en_US.json", "{\"hello\":\"world\"}")
                    .expect("valid resource"),
            )
            .with_resource(TextResource::new("prompts/default.md", "hi").expect("valid resource"));

        let first =
            bootstrap_text_resources_with_report(temp.path(), &manifest).expect("first bootstrap");
        assert_eq!(first.created_paths().len(), 2);
        assert!(temp.path().join("i18n").exists());
        assert!(temp.path().join("prompts").join("default.md").exists());

        let second =
            bootstrap_text_resources_with_report(temp.path(), &manifest).expect("second bootstrap");
        assert!(second.created.is_empty());
        assert_eq!(second.existing.len(), 2);
        assert!(second.created_directories.is_empty());
    }

    #[test]
    fn bootstrap_creates_missing_root_with_secure_open() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("nested").join("root");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hello").expect("valid resource"));

        let report =
            bootstrap_text_resources_with_report(&root, &manifest).expect("bootstrap nested root");

        assert!(root.join("default.md").is_file());
        assert_eq!(report.created, vec![PathBuf::from("default.md")]);
        assert_eq!(report.created_paths(), vec![root.join("default.md")]);
        assert_eq!(report.created_directories, vec![PathBuf::new()]);
        assert_eq!(
            report.created_directory_paths(),
            vec![temp.path().join("nested"), root]
        );
    }

    #[test]
    fn directories_to_prepare_deduplicates_resource_parents() {
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("shared/one.md", "one").expect("valid resource"))
            .with_resource(TextResource::new("shared/two.md", "two").expect("valid resource"))
            .with_resource(TextResource::new("nested/three.md", "three").expect("valid resource"));

        assert_eq!(directories_to_prepare(&manifest), vec!["shared", "nested"]);
    }

    #[test]
    fn rollback_created_resources_removes_files_and_directories() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("root");
        let nested = root.join("nested").join("deeper");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("default.md"), "hello").expect("write prompt");

        let mut report = BootstrapReport::new(root.clone());
        report.set_base(temp.path().to_path_buf());
        report
            .push_created_absolute(nested.join("default.md"))
            .expect("track created file");
        report.extend_created_directories_absolute(vec![root.clone(), root.join("nested"), nested]);

        rollback_created_resources(&report).expect("rollback should succeed");

        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rollback_created_resources_does_not_follow_replaced_parent_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        let root = temp.path().join("root");
        let nested = root.join("nested");
        let moved_nested = temp.path().join("moved_nested");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("managed.md"), "managed").expect("write managed file");
        fs::write(outside.path().join("managed.md"), "outside").expect("write outside file");

        let mut report = BootstrapReport::new(root.clone());
        report.set_base(temp.path().to_path_buf());
        report
            .push_created_absolute(nested.join("managed.md"))
            .expect("track created file");
        report.extend_created_directories_absolute(vec![root.clone(), nested.clone()]);

        fs::rename(&nested, &moved_nested).expect("move nested aside");
        symlink(outside.path(), &nested).expect("replace nested with symlink");

        let error = rollback_created_resources(&report).expect_err("rollback should fail safely");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read_to_string(outside.path().join("managed.md")).expect("outside file survives"),
            "outside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rollback_created_resources_handles_non_utf8_root_components() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join(OsString::from_vec(vec![b'r', 0xFF, b't']));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("managed.md"), "hello").expect("write managed file");

        let mut report = BootstrapReport::new(root.clone());
        report.set_base(temp.path().to_path_buf());
        report
            .push_created_absolute(nested.join("managed.md"))
            .expect("track created file");
        report.extend_created_directories_absolute(vec![root.clone(), nested]);

        rollback_created_resources(&report).expect("rollback should handle non-utf8 path");
        assert!(!root.exists());
    }

    #[test]
    fn bootstrap_rejects_paths_that_escape_root() {
        let temp = TempDir::new().expect("temp dir");
        let manifest =
            ResourceManifest::from_resources_unchecked(vec![TextResource::new_unchecked(
                "../escape.txt",
                "nope",
            )]);

        let err = bootstrap_text_resources(temp.path(), &manifest).expect_err("reject path escape");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn bootstrap_rejects_existing_directory_where_file_should_be() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("prompts").join("default.md")).expect("mkdir");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("prompts/default.md", "hi").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("directory should not be treated as existing file");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn bootstrap_rolls_back_earlier_files_when_a_later_resource_write_fails() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("blocked")).expect("mkdir blocked");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("created.md", "hi").expect("valid resource"))
            .with_resource(TextResource::new("blocked", "nope").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("blocked directory should fail resource write");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(!temp.path().join("created.md").exists());
    }

    #[test]
    fn bootstrap_only_rolls_back_directories_created_by_the_current_attempt() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("shared")).expect("mkdir shared");
        fs::create_dir_all(root.join("blocked")).expect("mkdir blocked");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("fresh/file.md", "hi").expect("valid resource"))
            .with_resource(TextResource::new("blocked", "nope").expect("valid resource"));

        let err = bootstrap_text_resources(&root, &manifest)
            .expect_err("blocked directory should fail resource write");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(root.exists());
        assert!(root.join("shared").is_dir());
        assert!(root.join("blocked").is_dir());
        assert!(!root.join("fresh").exists());
    }

    #[test]
    fn bootstrap_reports_file_parent_components_as_directories() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("nested"), "not a directory").expect("write blocking file");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("nested/file.md", "hello").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("file parent should be rejected");
        assert_eq!(err.kind(), io::ErrorKind::NotADirectory);
        assert!(err.to_string().contains("must be a directory"));
    }

    #[test]
    fn bootstrap_rejects_empty_path_components_before_writing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest =
            ResourceManifest::from_resources_unchecked(vec![TextResource::new_unchecked(
                "nested//file.txt",
                "two",
            )]);

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("empty path component should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("empty components"));
        assert!(
            std::fs::read_dir(temp.path())
                .expect("read temp dir")
                .next()
                .is_none()
        );
    }

    #[test]
    fn bootstrap_rejects_file_paths_that_conflict_with_descendants() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("nested", "parent").expect("valid resource"))
            .with_resource(TextResource::new("nested/child.txt", "child").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("file path conflict should fail");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(!temp.path().join("nested").exists());
    }

    #[test]
    fn bootstrap_rejects_non_adjacent_conflicting_manifest_paths_before_writing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("nested", "parent").expect("valid resource"))
            .with_resource(
                TextResource::new("nested!other/file.txt", "sibling").expect("valid resource"),
            )
            .with_resource(TextResource::new("nested/child.txt", "child").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("conflicting manifest paths should fail before writes");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(
            err.to_string()
                .contains("resource paths conflict in manifest")
        );
        assert!(
            std::fs::read_dir(temp.path())
                .expect("read temp dir")
                .next()
                .is_none()
        );
    }

    #[test]
    fn bootstrap_rejects_case_colliding_manifest_paths_before_writing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("Prompt.md", "one").expect("valid resource"))
            .with_resource(TextResource::new("prompt.md", "two").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("case-colliding manifest paths should fail");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("duplicate resource path"));
        assert!(
            std::fs::read_dir(temp.path())
                .expect("read temp dir")
                .next()
                .is_none()
        );
    }

    #[test]
    fn bootstrap_rejects_oversized_resource_contents_before_writing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest =
            ResourceManifest::from_resources_unchecked(vec![TextResource::new_unchecked(
                "huge.txt",
                "x".repeat(MAX_TEXT_RESOURCE_BYTES + 1),
            )]);

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("oversized resource should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeds size limit"));
        assert!(!temp.path().join("huge.txt").exists());
    }

    #[test]
    fn bootstrap_rejects_manifests_that_exceed_total_size_limit_before_writing() {
        let temp = TempDir::new().expect("temp dir");
        let resource_count = (MAX_TEXT_DIRECTORY_TOTAL_BYTES / MAX_TEXT_RESOURCE_BYTES) + 1;
        let mut manifest = ResourceManifest::new();
        for index in 0..resource_count {
            manifest = manifest.with_resource(
                TextResource::new(
                    format!("file-{index}.txt"),
                    "x".repeat(MAX_TEXT_RESOURCE_BYTES),
                )
                .expect("valid resource"),
            );
        }

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("oversized manifest should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("total size limit"));
        assert!(
            std::fs::read_dir(temp.path())
                .expect("read temp dir")
                .next()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_rejects_symlinked_parent_directory() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        symlink(&outside, temp.path().join("linked")).expect("create symlink");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("linked/file.txt", "hi").expect("valid resource"));

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("symlink parent should be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_rejects_symlinked_root_path() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        let root = temp.path().join("linked_root");
        symlink(outside.path(), &root).expect("create root symlink");

        let manifest = ResourceManifest::new()
            .with_resource(TextResource::new("default.md", "hi").expect("valid resource"));

        let err =
            bootstrap_text_resources(&root, &manifest).expect_err("symlinked root should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_rejects_backslash_components_for_loader_compatibility() {
        let temp = TempDir::new().expect("temp dir");
        let manifest =
            ResourceManifest::from_resources_unchecked(vec![TextResource::new_unchecked(
                r"a\b.txt", "bad",
            )]);

        let err = bootstrap_text_resources(temp.path(), &manifest)
            .expect_err("backslash component should be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
