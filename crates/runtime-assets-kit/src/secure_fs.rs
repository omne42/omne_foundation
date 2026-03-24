use std::io;
use std::path::Path;

#[cfg(any(feature = "i18n", feature = "prompts", test))]
use std::io::Write;
#[cfg(any(feature = "i18n", feature = "prompts", test))]
use std::path::PathBuf;

use omne_systems_fs_primitives::{
    self, DEFAULT_TEXT_FILE_BYTES_LIMIT, DEFAULT_TEXT_TREE_BYTES_LIMIT, Dir, EntryKind, File,
    MissingRootPolicy, ReadUtf8Error,
};

#[cfg(any(feature = "i18n", feature = "prompts", test))]
use crate::resource_path::relative_resource_components;
use crate::resource_path::validate_relative_resource_component;

pub(crate) const MAX_TEXT_RESOURCE_BYTES: usize = DEFAULT_TEXT_FILE_BYTES_LIMIT;
pub(crate) const MAX_TEXT_DIRECTORY_TOTAL_BYTES: usize = DEFAULT_TEXT_TREE_BYTES_LIMIT;

pub(crate) struct SecureRoot {
    dir: Dir,
}

impl SecureRoot {
    pub(crate) fn open(
        root: &Path,
        missing_root_policy: MissingRootPolicy,
    ) -> io::Result<Option<Self>> {
        omne_systems_fs_primitives::open_root(
            root,
            "resource root",
            missing_root_policy,
            |directory, component, _, error| {
                map_directory_component_error(directory, component, error)
            },
        )
        .map(|root| {
            root.map(|root| Self {
                dir: root.into_dir(),
            })
        })
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    pub(crate) fn open_with_report(
        root: &Path,
        missing_root_policy: MissingRootPolicy,
    ) -> io::Result<Option<(Self, Vec<PathBuf>)>> {
        omne_systems_fs_primitives::open_root_with_report(
            root,
            "resource root",
            missing_root_policy,
            |directory, component, _, error| {
                map_directory_component_error(directory, component, error)
            },
        )
        .map(|root| {
            root.map(|root| {
                let (root, created_directories) = root.into_parts();
                (
                    Self {
                        dir: root.into_dir(),
                    },
                    created_directories,
                )
            })
        })
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    pub(crate) fn create_directory_all(&self, relative_path: &str) -> io::Result<Vec<String>> {
        let components = relative_resource_components(relative_path, true)?;
        create_directory_chain(self.dir.try_clone()?, &components)
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    pub(crate) fn read_file_to_string(&self, relative_path: &str) -> io::Result<String> {
        let components = relative_resource_components(relative_path, false)?;
        let (parent, leaf) = split_leaf(&components)?;
        let directory = open_directory_chain(self.dir.try_clone()?, parent, false)?;
        if omne_systems_fs_primitives::entry_kind_at(&directory, Path::new(leaf))?
            != EntryKind::File
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("text resource file must be a regular file: {relative_path}"),
            ));
        }

        let mut file = open_regular_file_at(&directory, leaf)?;
        read_text_file(&mut file, relative_path)
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    pub(crate) fn write_file_if_absent(
        &self,
        relative_path: &str,
        contents: &str,
    ) -> io::Result<WriteResult> {
        let components = relative_resource_components(relative_path, false)?;
        let (parent, leaf) = split_leaf(&components)?;
        let directory = open_directory_chain(self.dir.try_clone()?, parent, false)?;

        match create_regular_file_at(&directory, leaf) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(contents.as_bytes()) {
                    drop(file);
                    return Err(rollback_partial_write(
                        &directory,
                        leaf,
                        relative_path,
                        error,
                    ));
                }
                Ok(WriteResult::Created)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if omne_systems_fs_primitives::entry_kind_at(&directory, Path::new(leaf))?
                    == EntryKind::File
                {
                    Ok(WriteResult::ExistingFile)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "resource target already exists but is not a file: {relative_path}"
                        ),
                    ))
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn walk_text_files(&self) -> io::Result<Vec<(String, String)>> {
        let mut files = Vec::new();
        let mut stack = vec![(Vec::<String>::new(), self.dir.try_clone()?)];
        let mut total_bytes = 0usize;

        while let Some((prefix, directory)) = stack.pop() {
            let mut names = read_directory_names(&directory)?;
            names.sort();

            for name in names {
                let kind = omne_systems_fs_primitives::entry_kind_at(&directory, Path::new(&name))?;
                match kind {
                    EntryKind::Symlink => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "text resource path cannot be a symlink: {}",
                                join_display_path(&prefix, &name)
                            ),
                        ));
                    }
                    EntryKind::Directory => {
                        let next = open_directory_component(&directory, Path::new(&name))?;
                        let mut next_prefix = prefix.clone();
                        next_prefix.push(name);
                        stack.push((next_prefix, next));
                    }
                    EntryKind::File => {
                        let mut file = open_regular_file_at(&directory, &name)?;
                        let relative_path = join_display_path(&prefix, &name);
                        let contents = read_text_file(&mut file, &relative_path)?;
                        total_bytes = total_bytes.saturating_add(contents.len());
                        validate_total_text_bytes(total_bytes)?;

                        let mut key_parts = prefix.clone();
                        key_parts.push(name);
                        files.push((key_parts.join("/"), contents));
                    }
                    EntryKind::Other => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "text resource path must be a regular file or directory: {}",
                                join_display_path(&prefix, &name)
                            ),
                        ));
                    }
                }
            }
        }

        Ok(files)
    }
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
fn rollback_partial_write(
    directory: &Dir,
    leaf: &str,
    relative_path: &str,
    write_error: io::Error,
) -> io::Error {
    match omne_systems_fs_primitives::remove_file_or_symlink_at(directory, Path::new(leaf)) {
        Ok(()) => write_error,
        Err(cleanup_error) => io::Error::new(
            cleanup_error.kind(),
            format!(
                "failed to roll back partially written resource {relative_path}: {cleanup_error}; original write error: {write_error}"
            ),
        ),
    }
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
pub(crate) enum WriteResult {
    Created,
    ExistingFile,
}

fn read_text_file(file: &mut File, relative_path: &str) -> io::Result<String> {
    match omne_systems_fs_primitives::read_utf8_limited(file, MAX_TEXT_RESOURCE_BYTES) {
        Ok(contents) => Ok(contents),
        Err(ReadUtf8Error::Io(error)) => Err(error),
        Err(ReadUtf8Error::TooLarge { bytes, max_bytes }) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "text resource file exceeds size limit ({bytes} > {max_bytes} bytes): {relative_path}"
            ),
        )),
        Err(ReadUtf8Error::InvalidUtf8(error)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("text resource file must be valid UTF-8: {relative_path} ({error})"),
        )),
    }
}

pub(crate) fn validate_total_text_bytes(total_bytes: usize) -> io::Result<()> {
    if total_bytes > MAX_TEXT_DIRECTORY_TOTAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "text resource directory exceeds total size limit ({} > {} bytes)",
                total_bytes, MAX_TEXT_DIRECTORY_TOTAL_BYTES
            ),
        ));
    }

    Ok(())
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
fn create_directory_chain(mut directory: Dir, components: &[&str]) -> io::Result<Vec<String>> {
    let mut created = Vec::new();
    let mut prefix = Vec::with_capacity(components.len());

    for component in components {
        let component = Path::new(component);
        match open_directory_component(&directory, component) {
            Ok(next) => {
                prefix.push(component.to_string_lossy().into_owned());
                directory = next;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let created_here = create_directory_component_if_missing(&directory, component)?;
                let next = open_directory_component(&directory, component)?;
                prefix.push(component.to_string_lossy().into_owned());
                if created_here {
                    created.push(prefix.join("/"));
                }
                directory = next;
            }
            Err(error) => return Err(error),
        }
    }

    Ok(created)
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
fn open_directory_chain(mut directory: Dir, components: &[&str], create: bool) -> io::Result<Dir> {
    for component in components {
        let component = Path::new(component);
        match open_directory_component(&directory, component) {
            Ok(next) => directory = next,
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                let _ = create_directory_component_if_missing(&directory, component)?;
                directory = open_directory_component(&directory, component)?;
            }
            Err(error) => return Err(error),
        }
    }

    Ok(directory)
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
fn split_leaf<'a>(components: &'a [&'a str]) -> io::Result<(&'a [&'a str], &'a str)> {
    let (leaf, parent) = components.split_last().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "resource path cannot be empty")
    })?;
    Ok((parent, leaf))
}

fn open_directory_component(directory: &Dir, component: &Path) -> io::Result<Dir> {
    omne_systems_fs_primitives::open_directory_component(directory, component)
        .map_err(|error| map_directory_component_error(directory, component, error))
}

fn open_regular_file_at(directory: &Dir, component: &str) -> io::Result<File> {
    match omne_systems_fs_primitives::open_regular_file_at(directory, Path::new(component)) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("text resource file must be a regular file: {component}"),
        )),
        Err(error) => Err(map_file_component_error(
            directory,
            Path::new(component),
            error,
        )),
    }
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
fn create_regular_file_at(directory: &Dir, component: &str) -> io::Result<File> {
    omne_systems_fs_primitives::create_regular_file_at(directory, Path::new(component))
        .map_err(|error| map_file_component_error(directory, Path::new(component), error))
}

#[cfg(any(feature = "i18n", feature = "prompts", test))]
fn create_directory_component_if_missing(directory: &Dir, component: &Path) -> io::Result<bool> {
    match directory.create_dir(component) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_directory_names(directory: &Dir) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for name in omne_systems_fs_primitives::read_directory_names(directory)? {
        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "text resource path is not valid UTF-8",
            )
        })?;
        validate_relative_resource_component(name, name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        names.push(name.to_string());
    }
    Ok(names)
}

fn join_display_path(prefix: &[String], leaf: &str) -> String {
    if prefix.is_empty() {
        leaf.to_string()
    } else {
        format!("{}/{}", prefix.join("/"), leaf)
    }
}

fn map_directory_component_error(directory: &Dir, component: &Path, error: io::Error) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidInput,
            "resource path must stay within root without crossing symlinks",
        ),
        Ok(metadata) if !metadata.is_dir() => io::Error::new(
            error.kind(),
            format!(
                "resource path component must be a directory: {}",
                component.display()
            ),
        ),
        _ => error,
    }
}

fn map_file_component_error(directory: &Dir, component: &Path, error: io::Error) -> io::Error {
    match directory.symlink_metadata(component) {
        Ok(metadata) if metadata.file_type().is_symlink() => io::Error::new(
            io::ErrorKind::InvalidInput,
            "resource path must stay within root without crossing symlinks",
        ),
        _ => error,
    }
}
