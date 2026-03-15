use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::RwLock;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextDirectory {
    entries: BTreeMap<String, String>,
}

impl TextDirectory {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn load(root: &Path) -> io::Result<Self> {
        if !root.exists() {
            return Ok(Self::default());
        }

        let mut entries = BTreeMap::new();
        load_directory_recursive(root, root, &mut entries)?;
        Ok(Self { entries })
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|s| s.as_str())
    }

    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
}

#[derive(Debug)]
pub struct GlobalTextDirectory {
    inner: RwLock<Option<TextDirectory>>,
}

impl GlobalTextDirectory {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }

    pub fn replace(&self, directory: TextDirectory) {
        *write_unpoisoned(&self.inner) = Some(directory);
    }

    pub fn clear(&self) {
        *write_unpoisoned(&self.inner) = None;
    }

    pub fn load_from_directory(&self, root: &Path) -> io::Result<()> {
        let directory = TextDirectory::load(root)?;
        self.replace(directory);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        read_unpoisoned(&self.inner)
            .as_ref()
            .and_then(|directory| directory.get(key))
            .map(ToOwned::to_owned)
    }

    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        read_unpoisoned(&self.inner)
            .as_ref()
            .map(TextDirectory::keys)
            .unwrap_or_default()
    }
}

fn load_directory_recursive(
    base: &Path,
    current: &Path,
    entries: &mut BTreeMap<String, String>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            load_directory_recursive(base, &path, entries)?;
            continue;
        }

        let relative = path
            .strip_prefix(base)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let key = relative.to_string_lossy().replace('\\', "/");
        let contents = fs::read_to_string(&path)?;
        entries.insert(key, contents);
    }
    Ok(())
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().expect("GlobalTextDirectory read lock poisoned")
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .expect("GlobalTextDirectory write lock poisoned")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn text_directory_loads_nested_files() {
        let temp = TempDir::new().expect("temp dir");
        let prompts_dir = temp.path().join("prompts");
        fs::create_dir_all(prompts_dir.join("nested")).expect("mkdir");
        fs::write(prompts_dir.join("default.md"), "hello").expect("write default");
        fs::write(prompts_dir.join("nested/system.md"), "world").expect("write nested");

        let directory = TextDirectory::load(&prompts_dir).expect("load");
        assert_eq!(directory.get("default.md"), Some("hello"));
        assert_eq!(directory.get("nested/system.md"), Some("world"));
        assert_eq!(
            directory.keys(),
            vec!["default.md".to_string(), "nested/system.md".to_string(),]
        );
    }

    #[test]
    fn global_text_directory_can_replace_and_clear() {
        let directory = GlobalTextDirectory::new();
        directory.replace(TextDirectory {
            entries: BTreeMap::from([("default.md".to_string(), "hello".to_string())]),
        });

        assert_eq!(directory.get("default.md"), Some("hello".to_string()));
        assert_eq!(directory.keys(), vec!["default.md".to_string()]);

        directory.clear();

        assert_eq!(directory.get("default.md"), None);
        assert!(directory.keys().is_empty());
    }
}
