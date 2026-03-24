use std::io;
use std::path::{Path, PathBuf};

use omne_systems_fs_primitives::{self, Dir, EntryKind, File, MissingRootPolicy, ReadUtf8Error};

use super::{
    DynamicCatalogError,
    locale_sources::{
        MAX_CATALOG_DIRECTORIES, MAX_LOCALE_SOURCE_BYTES, MAX_LOCALE_SOURCES,
        validate_catalog_directory_count, validate_catalog_directory_depth,
        validate_locale_source_limits,
    },
};

pub(super) fn read_locale_sources_from_directory(
    root: &Path,
) -> Result<Vec<(PathBuf, String)>, DynamicCatalogError> {
    let (root_path, root_dir) = open_catalog_root(root)?;
    let mut sources = Vec::new();
    let mut stack = vec![(PathBuf::new(), root_dir.try_clone()?, 0usize)];
    let mut directory_count = 0usize;
    let mut source_count = 0usize;
    let mut total_bytes = 0usize;

    while let Some((relative_prefix, directory, depth)) = stack.pop() {
        let current_path = root_path.join(&relative_prefix);
        let names = read_directory_names(
            &directory,
            &current_path,
            remaining_catalog_entry_budget(directory_count, source_count),
        )?;

        let mut nested_directories = Vec::new();
        for name in names {
            let relative_path = relative_prefix.join(&name);
            let display_path = root_path.join(&relative_path);
            let component = Path::new(&name);
            let kind = omne_systems_fs_primitives::entry_kind_at(&directory, component).map_err(
                |error| map_catalog_entry_access_error(&directory, component, &display_path, error),
            )?;

            match kind {
                EntryKind::Symlink => {
                    return Err(DynamicCatalogError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "catalog path cannot be a symlink: {}",
                            display_path.display()
                        ),
                    )));
                }
                EntryKind::Directory => {
                    let child_depth = depth + 1;
                    validate_catalog_directory_depth(&display_path, child_depth)?;
                    directory_count += 1;
                    validate_catalog_directory_count(directory_count)?;
                    let child_directory =
                        omne_systems_fs_primitives::open_directory_component(&directory, component)
                            .map_err(|error| {
                                map_catalog_entry_access_error(
                                    &directory,
                                    component,
                                    &display_path,
                                    error,
                                )
                            })?;
                    nested_directories.push((relative_path, child_directory, child_depth));
                }
                EntryKind::File => {
                    match relative_path.extension().and_then(|value| value.to_str()) {
                        Some("json") | None => {}
                        Some(_) => {
                            return Err(DynamicCatalogError::InvalidLocaleFileName(
                                display_path.display().to_string(),
                            ));
                        }
                    }

                    let mut file =
                        open_regular_file_at(&directory, component).map_err(|error| {
                            map_catalog_entry_access_error(
                                &directory,
                                component,
                                &display_path,
                                error,
                            )
                        })?;
                    let contents = read_locale_source(&mut file, &display_path)?;
                    source_count += 1;
                    total_bytes = total_bytes.saturating_add(contents.len());
                    validate_locale_source_limits(
                        &display_path,
                        source_count,
                        contents.len(),
                        total_bytes,
                    )?;
                    sources.push((display_path, contents));
                }
                EntryKind::Other => {
                    return Err(DynamicCatalogError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "catalog path must be a regular file or directory: {}",
                            display_path.display()
                        ),
                    )));
                }
            }
        }

        for entry in nested_directories.into_iter().rev() {
            stack.push(entry);
        }
    }

    Ok(sources)
}

fn open_catalog_root(root: &Path) -> io::Result<(PathBuf, Dir)> {
    let Some(root) = omne_systems_fs_primitives::open_root(
        root,
        "catalog root",
        MissingRootPolicy::Error,
        map_catalog_root_access_error,
    )?
    else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "catalog root does not exist",
        ));
    };

    Ok((root.path().to_path_buf(), root.into_dir()))
}

fn open_regular_file_at(directory: &Dir, component: &Path) -> io::Result<File> {
    match omne_systems_fs_primitives::open_regular_file_at(directory, component) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalog locale file must be a regular file",
        )),
        Err(error) => Err(error),
    }
}

fn read_locale_source(file: &mut File, path: &Path) -> Result<String, DynamicCatalogError> {
    match omne_systems_fs_primitives::read_utf8_limited(file, MAX_LOCALE_SOURCE_BYTES) {
        Ok(contents) => Ok(contents),
        Err(ReadUtf8Error::Io(error)) => Err(DynamicCatalogError::Io(error)),
        Err(ReadUtf8Error::TooLarge { bytes, max_bytes }) => {
            Err(DynamicCatalogError::LocaleSourceTooLarge {
                path: path.display().to_string(),
                bytes,
                max_bytes,
            })
        }
        Err(ReadUtf8Error::InvalidUtf8(error)) => Err(DynamicCatalogError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "catalog locale file must be valid UTF-8: {} ({error})",
                path.display()
            ),
        ))),
    }
}

fn remaining_catalog_entry_budget(directory_count: usize, source_count: usize) -> usize {
    MAX_CATALOG_DIRECTORIES.saturating_sub(directory_count)
        + MAX_LOCALE_SOURCES.saturating_sub(source_count)
}

fn read_directory_names(
    directory: &Dir,
    path: &Path,
    max_entries: usize,
) -> Result<Vec<String>, DynamicCatalogError> {
    let mut names = Vec::new();

    for name in directory.entries().map_err(DynamicCatalogError::Io)? {
        let name = name.map_err(DynamicCatalogError::Io)?.file_name();
        let name = name.to_str().ok_or_else(|| {
            DynamicCatalogError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("catalog path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        if name.contains('\\') {
            return Err(DynamicCatalogError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("catalog path component contains backslash: {name}"),
            )));
        }
        if name.contains(':') {
            return Err(DynamicCatalogError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("catalog path component contains colon: {name}"),
            )));
        }

        names.push(name.to_owned());
        if names.len() > max_entries {
            return Err(DynamicCatalogError::CatalogDirectoryTooWide {
                path: path.display().to_string(),
                entries: names.len(),
                max_entries,
            });
        }
    }

    names.sort();
    Ok(names)
}

fn map_catalog_root_access_error(
    directory: &Dir,
    component: &Path,
    path: &Path,
    error: io::Error,
) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "catalog root must not traverse symlinks: {}",
                path.display()
            ),
        ),
        Ok(_) => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("catalog root must be a directory: {}", path.display()),
        ),
        Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => io::Error::new(
            io::ErrorKind::NotFound,
            format!("catalog root does not exist: {}", path.display()),
        ),
        _ => error,
    }
}

fn map_catalog_entry_access_error(
    directory: &Dir,
    component: &Path,
    path: &Path,
    error: io::Error,
) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("catalog path cannot be a symlink: {}", path.display()),
        ),
        _ => error,
    }
}
