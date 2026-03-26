use std::ffi::OsString;
use std::io;
use std::path::Path;

use std::io::Write;
use std::path::PathBuf;

use omne_fs_primitives::{
    self, DEFAULT_TEXT_FILE_BYTES_LIMIT, DEFAULT_TEXT_TREE_BYTES_LIMIT, Dir, File,
    MissingRootPolicy, ReadUtf8Error,
};

use crate::resource_path::{
    materialize_resource_root, relative_resource_components, validate_relative_resource_component,
};
use crate::text_tree_scan::{TextTreeEntryKind, scan_text_tree};

pub const MAX_TEXT_RESOURCE_BYTES: usize = DEFAULT_TEXT_FILE_BYTES_LIMIT;
pub const MAX_TEXT_DIRECTORY_TOTAL_BYTES: usize = DEFAULT_TEXT_TREE_BYTES_LIMIT;

/// Scans a text directory with the same path-safety and file-content rules used
/// by `TextDirectory`.
///
/// `visit_directory` observes every directory, including the root, after its
/// validated entry names have been collected. `enter_directory` runs before a
/// nested directory is opened. `visit_file` receives normalized relative paths,
/// UTF-8 contents, and the depth of the containing directory.
pub fn scan_text_directory<
    E,
    T,
    VisitDirectory,
    EnterDirectory,
    VisitFile,
    MapFileTooLarge,
    MapDirectoryTooLarge,
>(
    root: &Path,
    mut visit_directory: VisitDirectory,
    mut enter_directory: EnterDirectory,
    mut visit_file: VisitFile,
    mut map_file_too_large: MapFileTooLarge,
    mut map_directory_too_large: MapDirectoryTooLarge,
) -> Result<Vec<T>, E>
where
    E: From<io::Error>,
    VisitDirectory: FnMut(&Path, usize, usize) -> Result<(), E>,
    EnterDirectory: FnMut(&Path, usize) -> Result<(), E>,
    VisitFile: FnMut(&Path, String, usize) -> Result<T, E>,
    MapFileTooLarge: FnMut(&Path, usize, usize) -> E,
    MapDirectoryTooLarge: FnMut(usize, usize) -> E,
{
    let root = materialize_resource_root(root).map_err(E::from)?;
    let Some(root) = SecureRoot::open(&root, MissingRootPolicy::ReturnNone).map_err(E::from)?
    else {
        return Err(E::from(io::Error::new(
            io::ErrorKind::NotFound,
            format!("resource root does not exist: {}", root.display()),
        )));
    };

    let SecureRoot { dir } = root;
    let mut total_bytes = 0usize;
    scan_text_tree(
        dir,
        |directory, relative_prefix, depth| {
            let names = read_directory_names(directory).map_err(E::from)?;
            visit_directory(relative_prefix, depth, names.len())?;
            Ok(names)
        },
        |directory, component, _| entry_kind_at(directory, component).map_err(E::from),
        |directory, component, relative_path, child_depth| {
            enter_directory(relative_path, child_depth)?;
            open_directory_component(directory, component).map_err(E::from)
        },
        |directory, component, relative_path, depth| {
            let component = component.to_str().expect("validated UTF-8 path component");
            let mut file = open_regular_file_at(directory, component).map_err(E::from)?;
            let contents =
                read_scanned_text_file(&mut file, relative_path, &mut map_file_too_large)?;
            total_bytes = total_bytes.saturating_add(contents.len());
            if total_bytes > MAX_TEXT_DIRECTORY_TOTAL_BYTES {
                return Err(map_directory_too_large(
                    total_bytes,
                    MAX_TEXT_DIRECTORY_TOTAL_BYTES,
                ));
            }
            visit_file(relative_path, contents, depth)
        },
        |relative_path| {
            E::from(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "text resource path cannot be a symlink: {}",
                    relative_path.display()
                ),
            ))
        },
        |relative_path| {
            E::from(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "text resource path must be a regular file or directory: {}",
                    relative_path.display()
                ),
            ))
        },
    )
}

pub(crate) struct SecureRoot {
    dir: Dir,
}

impl SecureRoot {
    pub(crate) fn open(
        root: &Path,
        missing_root_policy: MissingRootPolicy,
    ) -> io::Result<Option<Self>> {
        omne_fs_primitives::open_root(
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

    pub(crate) fn open_with_report(
        root: &Path,
        missing_root_policy: MissingRootPolicy,
    ) -> io::Result<Option<(Self, Vec<PathBuf>)>> {
        let root = materialize_resource_root(root)?;
        let (existing_root, managed_components) = split_existing_resource_root(&root)?;
        let existing = omne_fs_primitives::open_root(
            &existing_root,
            "resource root",
            MissingRootPolicy::Error,
            |directory, component, _, error| {
                map_directory_component_error(directory, component, error)
            },
        )?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "resource root base directory is missing",
            )
        })?;
        let mut directory = existing.into_dir();
        let mut current_path = existing_root;
        let mut created_directories = Vec::new();

        for component in managed_components {
            let component = Path::new(&component);
            match open_directory_component(&directory, component) {
                Ok(next) => {
                    current_path.push(component);
                    directory = next;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match missing_root_policy {
                        MissingRootPolicy::Create => {
                            let created =
                                create_directory_component_if_missing(&directory, component)?;
                            let next = open_directory_component(&directory, component)?;
                            current_path.push(component);
                            if created {
                                created_directories.push(current_path.clone());
                            }
                            directory = next;
                        }
                        MissingRootPolicy::ReturnNone => return Ok(None),
                        MissingRootPolicy::Error => {
                            return Err(map_directory_component_error(
                                &directory, component, error,
                            ));
                        }
                    }
                }
                Err(error) => {
                    return Err(map_directory_component_error(&directory, component, error));
                }
            }
        }

        Ok(Some((Self { dir: directory }, created_directories)))
    }

    pub(crate) fn create_directory_all(&self, relative_path: &str) -> io::Result<Vec<String>> {
        let components = relative_resource_components(relative_path, true)?;
        create_directory_chain(self.dir.try_clone()?, &components)
    }

    pub(crate) fn read_file_to_string(&self, relative_path: &str) -> io::Result<String> {
        let components = relative_resource_components(relative_path, false)?;
        let (parent, leaf) = split_leaf(&components)?;
        let directory = open_directory_chain(self.dir.try_clone()?, parent, false)?;
        if entry_kind_at(&directory, Path::new(leaf))? != TextTreeEntryKind::File {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("text resource file must be a regular file: {relative_path}"),
            ));
        }

        let mut file = open_regular_file_at(&directory, leaf)?;
        read_text_file(&mut file, relative_path)
    }

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
                if entry_kind_at(&directory, Path::new(leaf))? == TextTreeEntryKind::File {
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
        let mut total_bytes = 0usize;
        scan_text_tree(
            self.dir.try_clone()?,
            |directory, _, _| read_directory_names(directory),
            |directory, component, _| entry_kind_at(directory, component),
            |directory, component, _, _| open_directory_component(directory, component),
            |directory, component, relative_path, _| {
                let component = component.to_str().expect("validated UTF-8 path component");
                let mut file = open_regular_file_at(directory, component)?;
                let relative_path = relative_path.to_string_lossy().into_owned();
                let contents = read_text_file(&mut file, &relative_path)?;
                total_bytes = total_bytes.saturating_add(contents.len());
                validate_total_text_bytes(total_bytes)?;
                Ok((relative_path, contents))
            },
            |relative_path| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "text resource path cannot be a symlink: {}",
                        relative_path.display()
                    ),
                )
            },
            |relative_path| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "text resource path must be a regular file or directory: {}",
                        relative_path.display()
                    ),
                )
            },
        )
    }
}

fn rollback_partial_write(
    directory: &Dir,
    leaf: &str,
    relative_path: &str,
    write_error: io::Error,
) -> io::Error {
    match directory.remove_file(Path::new(leaf)) {
        Ok(()) => write_error,
        Err(cleanup_error) => io::Error::new(
            cleanup_error.kind(),
            format!(
                "failed to roll back partially written resource {relative_path}: {cleanup_error}; original write error: {write_error}"
            ),
        ),
    }
}

pub(crate) enum WriteResult {
    Created,
    ExistingFile,
}

fn read_text_file(file: &mut File, relative_path: &str) -> io::Result<String> {
    match omne_fs_primitives::read_utf8_limited(file, MAX_TEXT_RESOURCE_BYTES) {
        Ok(contents) => Ok(contents),
        Err(ReadUtf8Error::Io(error)) => Err(error),
        Err(ReadUtf8Error::TooLarge { bytes, max_bytes }) => Err(text_resource_too_large_error(
            relative_path,
            bytes,
            max_bytes,
        )),
        Err(ReadUtf8Error::InvalidUtf8(error)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("text resource file must be valid UTF-8: {relative_path} ({error})"),
        )),
    }
}

pub(crate) fn validate_total_text_bytes(total_bytes: usize) -> io::Result<()> {
    if total_bytes > MAX_TEXT_DIRECTORY_TOTAL_BYTES {
        return Err(text_directory_too_large_error(
            total_bytes,
            MAX_TEXT_DIRECTORY_TOTAL_BYTES,
        ));
    }

    Ok(())
}

fn read_scanned_text_file<E, MapFileTooLarge>(
    file: &mut File,
    relative_path: &Path,
    map_file_too_large: &mut MapFileTooLarge,
) -> Result<String, E>
where
    E: From<io::Error>,
    MapFileTooLarge: FnMut(&Path, usize, usize) -> E,
{
    match omne_fs_primitives::read_utf8_limited(file, MAX_TEXT_RESOURCE_BYTES) {
        Ok(contents) => Ok(contents),
        Err(ReadUtf8Error::Io(error)) => Err(E::from(error)),
        Err(ReadUtf8Error::TooLarge { bytes, max_bytes }) => {
            Err(map_file_too_large(relative_path, bytes, max_bytes))
        }
        Err(ReadUtf8Error::InvalidUtf8(error)) => Err(E::from(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "text resource file must be valid UTF-8: {} ({error})",
                relative_path.display()
            ),
        ))),
    }
}

fn text_resource_too_large_error(relative_path: &str, bytes: usize, max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "text resource file exceeds size limit ({bytes} > {max_bytes} bytes): {relative_path}"
        ),
    )
}

fn text_directory_too_large_error(total_bytes: usize, max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "text resource directory exceeds total size limit ({total_bytes} > {max_bytes} bytes)"
        ),
    )
}

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

fn split_leaf<'a>(components: &'a [&'a str]) -> io::Result<(&'a [&'a str], &'a str)> {
    let (leaf, parent) = components.split_last().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "resource path cannot be empty")
    })?;
    Ok((parent, leaf))
}

fn open_directory_component(directory: &Dir, component: &Path) -> io::Result<Dir> {
    omne_fs_primitives::open_directory_component(directory, component)
        .map_err(|error| map_directory_component_error(directory, component, error))
}

fn open_regular_file_at(directory: &Dir, component: &str) -> io::Result<File> {
    match omne_fs_primitives::open_regular_file_at(directory, Path::new(component)) {
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

fn create_regular_file_at(directory: &Dir, component: &str) -> io::Result<File> {
    omne_fs_primitives::create_regular_file_at(directory, Path::new(component))
        .map_err(|error| map_file_component_error(directory, Path::new(component), error))
}

fn create_directory_component_if_missing(directory: &Dir, component: &Path) -> io::Result<bool> {
    match directory.create_dir(component) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_directory_names(directory: &Dir) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in directory.entries()? {
        let name = entry?.file_name();
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

fn entry_kind_at(directory: &Dir, component: &Path) -> io::Result<TextTreeEntryKind> {
    let metadata = directory.symlink_metadata(component)?;
    let file_type = metadata.file_type();
    Ok(if file_type.is_symlink() {
        TextTreeEntryKind::Symlink
    } else if metadata.is_dir() {
        TextTreeEntryKind::Directory
    } else if metadata.is_file() {
        TextTreeEntryKind::File
    } else {
        TextTreeEntryKind::Other
    })
}

fn split_existing_resource_root(root: &Path) -> io::Result<(PathBuf, Vec<OsString>)> {
    let mut existing_root = PathBuf::new();
    let mut managed_components = Vec::new();
    let mut saw_root = false;
    let mut missing_started = false;

    for component in root.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                existing_root.push(component.as_os_str());
                saw_root = true;
            }
            std::path::Component::Normal(part) => {
                if missing_started {
                    managed_components.push(part.to_os_string());
                    continue;
                }

                let candidate = existing_root.join(part);
                match std::fs::symlink_metadata(&candidate) {
                    Ok(_) => existing_root = candidate,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        missing_started = true;
                        managed_components.push(part.to_os_string());
                    }
                    Err(error) => return Err(error),
                }
            }
            std::path::Component::CurDir | std::path::Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "resource root must be a normalized absolute path: {}",
                        root.display()
                    ),
                ));
            }
        }
    }

    if !saw_root {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "resource root must be a normalized absolute path: {}",
                root.display()
            ),
        ));
    }

    Ok((existing_root, managed_components))
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

#[cfg(test)]
mod tests {
    use super::scan_text_directory;
    use std::fs;
    use std::io;
    use tempfile::TempDir;

    #[test]
    fn scan_text_directory_visits_empty_directories_and_files() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("nested")).expect("mkdir nested");
        fs::create_dir_all(temp.path().join("spare")).expect("mkdir spare");
        fs::write(temp.path().join("nested").join("leaf.txt"), "hello").expect("write leaf");

        let mut visited_directories = Vec::new();
        let files = scan_text_directory(
            temp.path(),
            |relative_path, depth, entries| {
                visited_directories.push((relative_path.display().to_string(), depth, entries));
                Ok::<(), io::Error>(())
            },
            |_, _| Ok::<(), io::Error>(()),
            |relative_path, contents, depth| {
                Ok::<_, io::Error>((relative_path.display().to_string(), contents, depth))
            },
            |path, bytes, max_bytes| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "text resource file exceeds size limit ({bytes} > {max_bytes} bytes): {}",
                        path.display()
                    ),
                )
            },
            |bytes, max_bytes| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "text resource directory exceeds total size limit ({bytes} > {max_bytes} bytes)"
                    ),
                )
            },
        )
        .expect("scan text directory");

        assert_eq!(
            visited_directories,
            vec![
                ("".to_string(), 0, 2),
                ("nested".to_string(), 1, 1),
                ("spare".to_string(), 1, 0),
            ]
        );
        assert_eq!(
            files,
            vec![("nested/leaf.txt".to_string(), "hello".to_string(), 1)]
        );
    }
}
