use std::cell::Cell;
use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};

use i18n_kit::{
    DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy, Locale, MAX_LOCALE_SOURCES,
    validate_locale_source_limits, validate_locale_source_path,
};
use text_assets_kit::{
    BootstrapLoadError, ResourceManifest, TextDirectory, bootstrap_text_resources_then_load,
    materialize_resource_root, scan_text_directory,
};

const MAX_CATALOG_DIRECTORIES: usize = 2048;
const MAX_CATALOG_DIRECTORY_DEPTH: usize = 32;

#[derive(Debug)]
pub enum ResourceCatalogError {
    Bootstrap(io::Error),
    Load(DynamicCatalogError),
}

impl Display for ResourceCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap(error) => write!(f, "bootstrap text resources: {error}"),
            Self::Load(error) => write!(f, "load i18n catalog: {error}"),
        }
    }
}

impl std::error::Error for ResourceCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bootstrap(error) => Some(error),
            Self::Load(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub struct CatalogBootstrapCleanupError {
    load: DynamicCatalogError,
    rollback: io::Error,
}

impl CatalogBootstrapCleanupError {
    #[must_use]
    pub fn load_error(&self) -> &DynamicCatalogError {
        &self.load
    }

    #[must_use]
    pub fn rollback_error(&self) -> &io::Error {
        &self.rollback
    }
}

impl Display for CatalogBootstrapCleanupError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "catalog load error: {}; rollback failed: {}",
            self.load, self.rollback
        )
    }
}

impl StdError for CatalogBootstrapCleanupError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.load)
    }
}

fn bootstrap_i18n_catalog_with_loader<L>(
    root: &Path,
    manifest: &ResourceManifest,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
    load: L,
) -> Result<DynamicJsonCatalog, ResourceCatalogError>
where
    L: FnOnce(
        &Path,
        &[String],
        Locale,
        FallbackStrategy,
    ) -> Result<DynamicJsonCatalog, DynamicCatalogError>,
{
    validate_catalog_manifest(manifest, default_locale, fallback_strategy)
        .map_err(ResourceCatalogError::Load)?;
    match bootstrap_text_resources_then_load(root, manifest, |root, resource_paths| {
        load(root, resource_paths, default_locale, fallback_strategy)
    }) {
        Ok(catalog) => Ok(catalog),
        Err(BootstrapLoadError::Bootstrap(error)) => Err(ResourceCatalogError::Bootstrap(error)),
        Err(BootstrapLoadError::Load(error)) => Err(ResourceCatalogError::Load(error)),
        Err(BootstrapLoadError::Rollback { load, rollback }) => Err(
            ResourceCatalogError::Bootstrap(catalog_bootstrap_cleanup_error(load, rollback)),
        ),
    }
}

/// Bootstraps catalog resources under `root` and then rebuilds the catalog
/// from the managed files on disk.
///
/// Concurrent bootstrap attempts are serialized per materialized root, both
/// within the current process and across cooperating local processes that
/// resolve the same lock directory, so that rollback from one attempt cannot
/// invalidate another attempt's load.
pub fn bootstrap_i18n_catalog(
    root: impl AsRef<Path>,
    manifest: &ResourceManifest,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<DynamicJsonCatalog, ResourceCatalogError> {
    bootstrap_i18n_catalog_with_loader(
        root.as_ref(),
        manifest,
        default_locale,
        fallback_strategy,
        load_catalog_from_resource_files,
    )
}

/// Loads a dynamic i18n catalog from a filesystem directory.
///
/// This adapter belongs to `i18n-runtime-kit` because it owns the runtime
/// boundary around directory traversal, file validation, and path safety.
pub fn load_i18n_catalog_from_directory(
    root: impl AsRef<Path>,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<DynamicJsonCatalog, DynamicCatalogError> {
    let sources = read_catalog_sources_from_directory(root.as_ref())?;
    DynamicJsonCatalog::from_locale_sources(sources, default_locale, fallback_strategy)
}

/// Reloads an existing dynamic i18n catalog from a filesystem directory.
pub fn reload_i18n_catalog_from_directory(
    catalog: &DynamicJsonCatalog,
    root: impl AsRef<Path>,
) -> Result<(), DynamicCatalogError> {
    let sources = read_catalog_sources_from_directory(root.as_ref())?;
    catalog.reload_from_locale_sources(sources)
}

fn catalog_bootstrap_cleanup_error(load: DynamicCatalogError, rollback: io::Error) -> io::Error {
    io::Error::new(
        rollback.kind(),
        CatalogBootstrapCleanupError { load, rollback },
    )
}

fn validate_catalog_manifest(
    manifest: &ResourceManifest,
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<(), DynamicCatalogError> {
    DynamicJsonCatalog::from_locale_sources(
        manifest.resources().iter().map(|resource| {
            (
                PathBuf::from(resource.relative_path()),
                resource.contents().to_owned(),
            )
        }),
        default_locale,
        fallback_strategy,
    )
    .map(|_| ())
}

fn load_catalog_from_resource_files(
    root: &Path,
    resource_paths: &[String],
    default_locale: Locale,
    fallback_strategy: FallbackStrategy,
) -> Result<DynamicJsonCatalog, DynamicCatalogError> {
    let sources = load_catalog_resource_sources(root, resource_paths)?;
    DynamicJsonCatalog::from_locale_sources(sources, default_locale, fallback_strategy)
}

fn load_catalog_resource_sources(
    root: &Path,
    resource_paths: &[String],
) -> Result<Vec<(PathBuf, String)>, DynamicCatalogError> {
    let directory = TextDirectory::load_resource_files(root, resource_paths)
        .map_err(DynamicCatalogError::Io)?;

    let mut sources = Vec::with_capacity(resource_paths.len());
    let mut source_count = 0usize;
    let mut total_bytes = 0usize;
    for relative_path in resource_paths {
        let contents = directory.get(relative_path).ok_or_else(|| {
            DynamicCatalogError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("resource root is missing managed file: {relative_path}"),
            ))
        })?;
        source_count += 1;
        total_bytes = total_bytes.saturating_add(contents.len());
        validate_locale_source_limits(
            Path::new(relative_path),
            source_count,
            contents.len(),
            total_bytes,
        )?;
        sources.push((PathBuf::from(relative_path), contents.to_owned()));
    }
    Ok(sources)
}

fn read_catalog_sources_from_directory(
    root: &Path,
) -> Result<Vec<(PathBuf, String)>, DynamicCatalogError> {
    let root = materialize_resource_root(root)?;
    let directory_count = Cell::new(0usize);
    let source_count = Cell::new(0usize);
    let total_bytes = Cell::new(0usize);

    scan_text_directory(
        &root,
        |relative_path, _, entries| {
            let display_path = root.join(relative_path);
            validate_catalog_directory_width(
                &display_path,
                entries,
                remaining_catalog_entry_budget(directory_count.get(), source_count.get()),
            )
        },
        |relative_path, child_depth| {
            let display_path = root.join(relative_path);
            validate_catalog_directory_depth(&display_path, child_depth)?;
            let next_directory_count = directory_count.get() + 1;
            validate_catalog_directory_count(next_directory_count)?;
            directory_count.set(next_directory_count);
            Ok(())
        },
        |relative_path, contents, _| {
            let display_path = root.join(relative_path);
            validate_locale_source_path(&display_path)?;
            let next_source_count = source_count.get() + 1;
            let next_total_bytes = total_bytes.get().saturating_add(contents.len());
            validate_locale_source_limits(
                &display_path,
                next_source_count,
                contents.len(),
                next_total_bytes,
            )?;
            source_count.set(next_source_count);
            total_bytes.set(next_total_bytes);
            Ok((display_path, contents))
        },
        |relative_path, bytes, max_bytes| DynamicCatalogError::LocaleSourceTooLarge {
            path: root.join(relative_path).display().to_string(),
            bytes,
            max_bytes,
        },
        |bytes, max_bytes| DynamicCatalogError::CatalogTooLarge { bytes, max_bytes },
    )
}

fn remaining_catalog_entry_budget(directory_count: usize, source_count: usize) -> usize {
    MAX_CATALOG_DIRECTORIES.saturating_sub(directory_count)
        + MAX_LOCALE_SOURCES.saturating_sub(source_count)
}

fn validate_catalog_directory_width(
    path: &Path,
    entries: usize,
    max_entries: usize,
) -> Result<(), DynamicCatalogError> {
    if entries > max_entries {
        return Err(DynamicCatalogError::CatalogDirectoryTooWide {
            path: path.display().to_string(),
            entries,
            max_entries,
        });
    }

    Ok(())
}

fn validate_catalog_directory_count(count: usize) -> Result<(), DynamicCatalogError> {
    if count > MAX_CATALOG_DIRECTORIES {
        return Err(DynamicCatalogError::TooManyCatalogDirectories {
            max: MAX_CATALOG_DIRECTORIES,
        });
    }

    Ok(())
}

fn validate_catalog_directory_depth(path: &Path, depth: usize) -> Result<(), DynamicCatalogError> {
    if depth > MAX_CATALOG_DIRECTORY_DEPTH {
        return Err(DynamicCatalogError::CatalogDirectoryTooDeep {
            path: path.display().to_string(),
            depth,
            max_depth: MAX_CATALOG_DIRECTORY_DEPTH,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CurrentDirGuard;
    use i18n_kit::{Catalog, TranslationCatalog};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;
    use text_assets_kit::{MAX_TEXT_DIRECTORY_TOTAL_BYTES, MAX_TEXT_RESOURCE_BYTES, TextResource};

    #[cfg(unix)]
    fn short_tempdir_for_unix_socket() -> TempDir {
        tempfile::Builder::new()
            .prefix("of-sock-")
            .rand_bytes(3)
            .tempdir_in("/var/tmp")
            .expect("short temp dir")
    }

    #[test]
    fn resource_backed_catalog_rebuilds_snapshot_from_current_disk_state() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("en_US.json", r#"{"greeting":"hello"}"#).expect("valid resource"),
        );

        let first = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            first.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );

        std::fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hi"}"#)
            .expect("rewrite locale file");
        let second = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("rebuild catalog");

        assert_eq!(
            second.get_text(Locale::EN_US, "greeting"),
            Some("hi".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_failed_rebuild_keeps_previous_snapshot() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("en_US.json", r#"{"greeting":"hello"}"#).expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );

        std::fs::write(temp.path().join("en_US.json"), "{").expect("write invalid locale");
        let error = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid json rebuild should fail");
        assert!(matches!(
            error,
            ResourceCatalogError::Load(DynamicCatalogError::LocaleSourceJson { .. })
        ));
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_rebuild_rejects_oversized_locale_before_parsing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("en_US.json", r#"{"greeting":"hello"}"#).expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");

        std::fs::write(
            temp.path().join("en_US.json"),
            vec![b'x'; MAX_TEXT_RESOURCE_BYTES + 1],
        )
        .expect("write oversized locale");

        let error = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("oversized locale rebuild should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::Io(error)) = error else {
            panic!("expected io error for oversized locale file");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds size limit"));
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_rejects_catalogs_that_exceed_total_size_limit() {
        let temp = TempDir::new().expect("temp dir");
        let locales = [
            "en_US", "fr_FR", "de_DE", "es_ES", "it_IT", "ja_JP", "ko_KR", "pt_BR", "zh_CN",
        ];
        let mut manifest = ResourceManifest::new();
        for locale in locales {
            manifest = manifest.with_resource(
                TextResource::new(
                    format!("{locale}.json"),
                    format!(r#"{{"greeting":"{}"}}"#, "x".repeat(950_000)),
                )
                .expect("valid resource"),
            );
        }

        let error = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("oversized catalog should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::CatalogTooLarge { bytes, max_bytes }) =
            error
        else {
            panic!("expected load total size limit error");
        };
        assert!(bytes > max_bytes);
    }

    #[test]
    fn resource_backed_catalog_rebuilds_from_absolute_root_across_cwd_changes() {
        let cwd = CurrentDirGuard::new();
        let temp = TempDir::new().expect("temp dir");
        let workspace_a = temp.path().join("workspace_a");
        let workspace_b = temp.path().join("workspace_b");
        let root = workspace_a.join("catalog");
        std::fs::create_dir_all(&workspace_a).expect("mkdir workspace_a");
        std::fs::create_dir_all(&workspace_b).expect("mkdir workspace_b");
        cwd.set(&workspace_a);

        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("en_US.json", r#"{"greeting":"hello"}"#).expect("valid resource"),
        );
        let catalog = bootstrap_i18n_catalog(
            PathBuf::from("catalog"),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");

        cwd.set(&workspace_b);
        std::fs::write(root.join("en_US.json"), r#"{"greeting":"hi"}"#)
            .expect("rewrite locale file");
        let rebuilt =
            bootstrap_i18n_catalog(&root, &manifest, Locale::EN_US, FallbackStrategy::Both)
                .expect("rebuild catalog");

        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(
            rebuilt.get_text(Locale::EN_US, "greeting"),
            Some("hi".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_loads_nested_locale_files() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("i18n/en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(catalog.available_locales(), vec![Locale::EN_US]);
    }

    #[test]
    fn resource_backed_catalog_ignores_unmanaged_root_json_files() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("notes.json"), r#"{"ignore":"me"}"#)
            .expect("write unrelated json");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("i18n/en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );

        let catalog = bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("bootstrap catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
        assert_eq!(catalog.available_locales(), vec![Locale::EN_US]);
    }

    #[test]
    fn resource_backed_catalog_errors_when_default_locale_is_missing() {
        let temp = TempDir::new().expect("temp dir");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("zh_CN.json", r#"{"greeting":"nihao"}"#).expect("valid resource"),
        );

        let err = match bootstrap_i18n_catalog(
            temp.path(),
            &manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        ) {
            Ok(_) => panic!("missing default locale should fail"),
            Err(err) => err,
        };
        let ResourceCatalogError::Load(DynamicCatalogError::MissingDefaultLocale(locale)) = err
        else {
            panic!("expected missing default locale load error");
        };
        assert_eq!(locale, Locale::EN_US);
    }

    #[test]
    fn resource_backed_catalog_bootstrap_rejects_invalid_manifest_without_writing_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let invalid_manifest = ResourceManifest::new()
            .with_resource(TextResource::new("i18n/en_US.json", "{").expect("valid resource path"));

        let err = bootstrap_i18n_catalog(
            &root,
            &invalid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid catalog json should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::LocaleSourceJson { path, .. }) = err
        else {
            panic!("expected json load error");
        };
        assert_eq!(path, "i18n/en_US.json");
        assert!(!root.exists());

        let valid_manifest = ResourceManifest::new().with_resource(
            TextResource::new("i18n/en_US.json", r#"{"greeting":"hello"}"#)
                .expect("valid resource"),
        );
        let catalog = bootstrap_i18n_catalog(
            &root,
            &valid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("second bootstrap should recover");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn resource_backed_catalog_bootstrap_rejects_invalid_template_without_writing_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let invalid_manifest = ResourceManifest::new().with_resource(
            TextResource::new("i18n/en_US.json", r#"{"greeting":"hello {name"}"#)
                .expect("valid resource path"),
        );

        let err = bootstrap_i18n_catalog(
            &root,
            &invalid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid catalog template should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::LocaleSourceJson { path, error }) = err
        else {
            panic!("expected template validation load error");
        };
        assert_eq!(path, "i18n/en_US.json");
        assert!(
            error
                .to_string()
                .contains("invalid catalog template for greeting: unclosed placeholder")
        );
        assert!(!root.exists());
    }

    #[test]
    fn resource_backed_catalog_rejects_invalid_locale_file_name_without_writing_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let invalid_manifest = ResourceManifest::new().with_resource(
            TextResource::new("i18n/not-a-locale.txt", r#"{"greeting":"hello"}"#)
                .expect("valid resource path"),
        );

        let err = bootstrap_i18n_catalog(
            &root,
            &invalid_manifest,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid locale file name should fail");
        let ResourceCatalogError::Load(DynamicCatalogError::InvalidLocaleFileName(path)) = err
        else {
            panic!("expected invalid locale file name load error");
        };
        assert_eq!(path, "i18n/not-a-locale.txt");
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn resource_backed_catalog_rebuild_rejects_symlinked_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("catalog");
        let backup = temp.path().join("catalog_real");
        let outside = TempDir::new().expect("outside dir");
        let manifest = ResourceManifest::new().with_resource(
            TextResource::new("en_US.json", r#"{"greeting":"hello"}"#).expect("valid resource"),
        );
        let catalog =
            bootstrap_i18n_catalog(&root, &manifest, Locale::EN_US, FallbackStrategy::Both)
                .expect("bootstrap catalog");

        std::fs::rename(&root, &backup).expect("move root aside");
        std::fs::write(
            outside.path().join("en_US.json"),
            r#"{"greeting":"outside"}"#,
        )
        .expect("write outside locale");
        symlink(outside.path(), &root).expect("symlink root");

        let err = bootstrap_i18n_catalog(&root, &manifest, Locale::EN_US, FallbackStrategy::Both)
            .expect_err("symlinked root should fail");
        let ResourceCatalogError::Bootstrap(error) = err else {
            panic!("expected bootstrap io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    fn load_directory_catalog(root: &Path) -> Result<DynamicJsonCatalog, DynamicCatalogError> {
        load_i18n_catalog_from_directory(root, Locale::EN_US, FallbackStrategy::Both)
    }

    fn generated_locale(index: usize) -> String {
        let first = ((index / (26 * 26)) % 26) as u8 + b'a';
        let second = ((index / 26) % 26) as u8 + b'a';
        let third = (index % 26) as u8 + b'a';
        String::from_utf8(vec![first, second, third]).expect("generated locale should be ASCII")
    }

    #[test]
    fn directory_catalog_loads_nested_locale_files() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("nested")).expect("mkdir");
        fs::write(
            temp.path().join("nested").join("en_US.json"),
            r#"{"greeting":"hello"}"#,
        )
        .expect("write nested locale");

        let catalog = load_directory_catalog(temp.path()).expect("load nested catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn directory_catalog_reload_swaps_in_new_snapshot() {
        let temp = TempDir::new().expect("temp dir");
        let locale_path = temp.path().join("en_US.json");
        fs::write(&locale_path, r#"{"greeting":"hello"}"#).expect("write initial locale");

        let catalog = load_directory_catalog(temp.path()).expect("load catalog");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );

        fs::write(&locale_path, r#"{"greeting":"hi"}"#).expect("rewrite locale");
        reload_i18n_catalog_from_directory(&catalog, temp.path()).expect("reload catalog");

        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hi".to_string())
        );
    }

    #[test]
    fn directory_catalog_rejects_excessive_directory_depth() {
        let temp = TempDir::new().expect("temp dir");
        let mut deepest = temp.path().to_path_buf();
        for index in 0..=MAX_CATALOG_DIRECTORY_DEPTH {
            deepest = deepest.join(format!("nested_{index:02}"));
            fs::create_dir_all(&deepest).expect("mkdir nested");
        }
        fs::write(deepest.join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");

        let err =
            load_directory_catalog(temp.path()).expect_err("overly deep catalogs should fail");
        assert!(matches!(
            err,
            DynamicCatalogError::CatalogDirectoryTooDeep {
                depth,
                max_depth,
                ..
            } if depth == MAX_CATALOG_DIRECTORY_DEPTH + 1
                && max_depth == MAX_CATALOG_DIRECTORY_DEPTH
        ));
    }

    #[test]
    fn directory_catalog_rejects_excessive_directory_count() {
        let temp = TempDir::new().expect("temp dir");
        for index in 0..=MAX_CATALOG_DIRECTORIES {
            let dir = temp.path().join(format!("dir_{index:04}"));
            fs::create_dir_all(&dir).expect("mkdir sibling");
            if index == 0 {
                fs::write(dir.join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");
            }
        }

        let err = load_directory_catalog(temp.path())
            .expect_err("catalogs with too many directories should fail");
        assert!(matches!(
            err,
            DynamicCatalogError::TooManyCatalogDirectories { max }
                if max == MAX_CATALOG_DIRECTORIES
        ));
    }

    #[test]
    fn directory_catalog_errors_when_default_locale_is_missing() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("zh_CN.json"), r#"{"greeting":"nihao"}"#).expect("write locale");

        let err =
            load_directory_catalog(temp.path()).expect_err("missing default locale should fail");
        let DynamicCatalogError::MissingDefaultLocale(locale) = err else {
            panic!("expected missing default locale error");
        };
        assert_eq!(locale, Locale::EN_US);
    }

    #[test]
    fn directory_catalog_errors_on_duplicate_locale_files() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("nested")).expect("mkdir");
        fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hello"}"#)
            .expect("write root locale");
        fs::write(
            temp.path().join("nested").join("en_US.json"),
            r#"{"greeting":"hi"}"#,
        )
        .expect("write nested locale");

        let err = load_directory_catalog(temp.path()).expect_err("duplicate locale should fail");
        assert!(matches!(
            err,
            DynamicCatalogError::DuplicateLocaleFile { .. }
        ));
    }

    #[test]
    fn directory_catalog_reports_duplicate_locale_files_in_stable_path_order() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("a")).expect("mkdir a");
        fs::create_dir_all(temp.path().join("b")).expect("mkdir b");
        fs::write(
            temp.path().join("a").join("en_US.json"),
            r#"{"greeting":"hello"}"#,
        )
        .expect("write a locale");
        fs::write(
            temp.path().join("b").join("en_US.json"),
            r#"{"greeting":"hi"}"#,
        )
        .expect("write b locale");

        let err = load_directory_catalog(temp.path()).expect_err("duplicate locale should fail");
        let DynamicCatalogError::DuplicateLocaleFile {
            first_path,
            second_path,
            ..
        } = err
        else {
            panic!("expected duplicate locale file error");
        };

        assert!(
            Path::new(&first_path).ends_with(Path::new("a").join("en_US.json")),
            "{first_path}"
        );
        assert!(
            Path::new(&second_path).ends_with(Path::new("b").join("en_US.json")),
            "{second_path}"
        );
    }

    #[test]
    fn directory_catalog_errors_on_invalid_locale_file_name() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hello"}"#)
            .expect("write default locale");
        fs::write(
            temp.path().join("definitely-not-a-locale.json"),
            r#"{"greeting":"bad"}"#,
        )
        .expect("write invalid locale file");

        let err = load_directory_catalog(temp.path()).expect_err("invalid locale file should fail");
        assert!(matches!(err, DynamicCatalogError::InvalidLocaleFileName(_)));
    }

    #[test]
    fn directory_catalog_accepts_extensionless_names() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("en_US"), r#"{"greeting":"hello"}"#).expect("write locale");

        let catalog =
            load_directory_catalog(temp.path()).expect("extensionless locale file should load");
        assert_eq!(
            catalog.get_text(Locale::EN_US, "greeting"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn directory_catalog_rejects_non_json_extensions() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("en_US.txt"), r#"{"greeting":"hello"}"#).expect("write locale");

        let err = load_directory_catalog(temp.path())
            .expect_err("unexpected extension should be rejected");
        assert!(matches!(
            err,
            DynamicCatalogError::InvalidLocaleFileName(path) if path.ends_with("en_US.txt")
        ));
    }

    #[test]
    fn directory_catalog_errors_when_root_directory_is_missing() {
        let temp = TempDir::new().expect("temp dir");
        let missing = temp.path().join("missing");

        let error = load_directory_catalog(&missing).expect_err("missing root should fail");
        let DynamicCatalogError::Io(error) = error else {
            panic!("expected io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn directory_catalog_errors_on_duplicate_catalog_keys_in_file() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(
            temp.path().join("en_US.json"),
            r#"{"greeting":"hello","greeting":"hi"}"#,
        )
        .expect("write locale");

        let err =
            load_directory_catalog(temp.path()).expect_err("duplicate catalog keys should fail");
        assert!(matches!(
            err,
            DynamicCatalogError::LocaleSourceJson { path, error }
                if path.ends_with("en_US.json")
                    && error.to_string().contains("duplicate catalog key: greeting")
        ));
    }

    #[test]
    fn directory_catalog_rejects_oversized_locale_source() {
        let temp = TempDir::new().expect("temp dir");
        let oversized = "x".repeat(MAX_TEXT_RESOURCE_BYTES);
        let content = format!(r#"{{"greeting":"{oversized}"}}"#);
        fs::write(temp.path().join("en_US.json"), content).expect("write oversized locale");

        let err =
            load_directory_catalog(temp.path()).expect_err("oversized locale source should fail");
        assert!(matches!(
            err,
            DynamicCatalogError::LocaleSourceTooLarge { max_bytes, .. }
                if max_bytes == MAX_TEXT_RESOURCE_BYTES
        ));
    }

    #[test]
    fn directory_catalog_rejects_catalogs_that_exceed_total_size_limit() {
        let temp = TempDir::new().expect("temp dir");
        let payload = "x".repeat(MAX_TEXT_RESOURCE_BYTES / 2);
        let template = format!(r#"{{"greeting":"{payload}"}}"#);
        let mut total_bytes = 0usize;
        let mut index = 0usize;
        while total_bytes <= MAX_TEXT_DIRECTORY_TOTAL_BYTES {
            let locale = if index == 0 {
                "en_US".to_string()
            } else {
                generated_locale(index)
            };
            total_bytes += template.len();
            fs::write(temp.path().join(format!("{locale}.json")), &template).expect("write locale");
            index += 1;
        }

        let err =
            load_directory_catalog(temp.path()).expect_err("catalog total size should be capped");
        assert!(matches!(
            err,
            DynamicCatalogError::CatalogTooLarge { max_bytes, .. }
                if max_bytes == MAX_TEXT_DIRECTORY_TOTAL_BYTES
        ));
    }

    #[cfg(unix)]
    #[test]
    fn directory_catalog_rejects_symlinked_locale_file() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside_dir = TempDir::new().expect("outside dir");
        let outside = outside_dir.path().join("en_US.json");
        fs::write(&outside, r#"{"greeting":"hello"}"#).expect("write outside locale");
        symlink(&outside, temp.path().join("en_US.json")).expect("create symlink");

        let error = load_directory_catalog(temp.path())
            .expect_err("symlinked locale file should be rejected");
        let DynamicCatalogError::Io(error) = error else {
            panic!("expected io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn directory_catalog_rejects_non_utf8_path_components() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("temp dir");
        let invalid = OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        let nested = temp.path().join(&invalid);
        fs::create_dir_all(&nested).expect("mkdir invalid path");
        fs::write(nested.join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");

        let error = load_directory_catalog(temp.path()).expect_err("non-utf8 path should fail");
        let DynamicCatalogError::Io(error) = error else {
            panic!("expected io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("not valid UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn directory_catalog_rejects_socket_entries() {
        use std::os::unix::net::UnixListener;

        let temp = short_tempdir_for_unix_socket();
        fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");
        let socket_path = temp.path().join("catalog.sock");
        let _listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping directory_catalog_rejects_socket_entries: unix socket bind not permitted in this environment: {err}"
                );
                return;
            }
            Err(err) => panic!("bind socket: {err}"),
        };

        let error = load_directory_catalog(temp.path()).expect_err("socket entries should fail");
        let DynamicCatalogError::Io(error) = error else {
            panic!("expected io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("regular file or directory"));
    }

    #[cfg(unix)]
    #[test]
    fn directory_catalog_rejects_symlinked_root_path() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        fs::write(outside.path().join("en_US.json"), r#"{"greeting":"hello"}"#)
            .expect("write locale");
        let root = temp.path().join("linked_root");
        symlink(outside.path(), &root).expect("create root symlink");

        let error = load_directory_catalog(&root).expect_err("symlinked root should fail");
        let DynamicCatalogError::Io(error) = error else {
            panic!("expected io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn directory_catalog_rejects_root_path_with_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        fs::create_dir_all(outside.path().join("nested")).expect("mkdir nested");
        fs::write(
            outside.path().join("nested").join("en_US.json"),
            r#"{"greeting":"hello"}"#,
        )
        .expect("write locale");
        symlink(outside.path(), temp.path().join("linked")).expect("create ancestor symlink");
        let root = temp.path().join("linked").join("nested");

        let error = load_directory_catalog(&root).expect_err("symlinked ancestor should fail");
        let DynamicCatalogError::Io(error) = error else {
            panic!("expected io error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn catalog_bootstrap_cleanup_error_preserves_both_failures() {
        let load_error = DynamicJsonCatalog::from_locale_sources(
            [(PathBuf::from("en_US.json"), "{".to_string())],
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect_err("invalid catalog json should fail");
        let error =
            catalog_bootstrap_cleanup_error(load_error, io::Error::other("rollback failed"));

        let cleanup = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<CatalogBootstrapCleanupError>())
            .expect("wrapped cleanup error");
        assert!(matches!(
            cleanup.load_error(),
            DynamicCatalogError::LocaleSourceJson { .. }
        ));
        assert_eq!(cleanup.rollback_error().to_string(), "rollback failed");
        assert!(matches!(
            cleanup
                .source()
                .expect("load source")
                .downcast_ref::<DynamicCatalogError>(),
            Some(DynamicCatalogError::LocaleSourceJson { .. })
        ));
        assert!(cleanup.to_string().contains("catalog load error:"));
        assert!(
            cleanup
                .to_string()
                .contains("rollback failed: rollback failed")
        );
    }
}
