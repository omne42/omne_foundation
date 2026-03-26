use std::path::{Path, PathBuf};

use omne_fs_primitives::Dir;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextTreeEntryKind {
    Symlink,
    Directory,
    File,
    Other,
}

pub(crate) fn scan_text_tree<E, T, ReadDirectoryNames, EntryKindAt, OpenDirectory, VisitFile>(
    root_dir: Dir,
    mut read_directory_names: ReadDirectoryNames,
    mut entry_kind_at: EntryKindAt,
    mut open_directory: OpenDirectory,
    mut visit_file: VisitFile,
    mut symlink_error: impl FnMut(&Path) -> E,
    mut other_error: impl FnMut(&Path) -> E,
) -> Result<Vec<T>, E>
where
    ReadDirectoryNames: FnMut(&Dir, &Path, usize) -> Result<Vec<String>, E>,
    EntryKindAt: FnMut(&Dir, &Path, &Path) -> Result<TextTreeEntryKind, E>,
    OpenDirectory: FnMut(&Dir, &Path, &Path, usize) -> Result<Dir, E>,
    VisitFile: FnMut(&Dir, &Path, &Path, usize) -> Result<T, E>,
{
    let mut collected = Vec::new();
    let mut stack = vec![(PathBuf::new(), root_dir, 0usize)];

    while let Some((relative_prefix, directory, depth)) = stack.pop() {
        let mut names = read_directory_names(&directory, relative_prefix.as_path(), depth)?;
        names.sort();

        let mut nested_directories = Vec::new();
        for name in names {
            let component = Path::new(&name);
            let relative_path = relative_prefix.join(component);

            match entry_kind_at(&directory, component, &relative_path)? {
                TextTreeEntryKind::Symlink => return Err(symlink_error(&relative_path)),
                TextTreeEntryKind::Directory => {
                    let child_directory =
                        open_directory(&directory, component, &relative_path, depth + 1)?;
                    nested_directories.push((relative_path, child_directory, depth + 1));
                }
                TextTreeEntryKind::File => {
                    collected.push(visit_file(&directory, component, &relative_path, depth)?);
                }
                TextTreeEntryKind::Other => return Err(other_error(&relative_path)),
            }
        }

        for entry in nested_directories.into_iter().rev() {
            stack.push(entry);
        }
    }

    Ok(collected)
}
