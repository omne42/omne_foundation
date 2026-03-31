use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[must_use]
pub fn filesystem_is_case_sensitive(path: &Path) -> bool {
    #[cfg(unix)]
    {
        unix_filesystem_is_case_sensitive(path)
    }

    #[cfg(windows)]
    {
        let _ = path;
        false
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = path;
        true
    }
}

#[cfg(unix)]
fn unix_filesystem_is_case_sensitive(path: &Path) -> bool {
    let Some(probe_path) = case_sensitivity_probe_path(path) else {
        return true;
    };

    let Ok(existing_metadata) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(probe_metadata) = std::fs::metadata(&probe_path) else {
        return true;
    };

    existing_metadata.dev() != probe_metadata.dev()
        || existing_metadata.ino() != probe_metadata.ino()
}

#[cfg(unix)]
fn case_sensitivity_probe_path(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    if let Some(variant) = case_variant_component(file_name) {
        return Some(path.with_file_name(variant));
    }

    let parent = path.parent()?;
    let prefix = case_sensitivity_probe_path(parent)?;
    Some(prefix.join(file_name))
}

#[cfg(unix)]
fn case_variant_component(component: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let bytes = component.as_bytes();
    for (index, byte) in bytes.iter().copied().enumerate() {
        let replacement = match byte {
            b'a'..=b'z' => byte.to_ascii_uppercase(),
            b'A'..=b'Z' => byte.to_ascii_lowercase(),
            _ => continue,
        };

        let mut variant = bytes.to_vec();
        variant[index] = replacement;
        return Some(std::ffi::OsString::from_vec(variant));
    }

    None
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::filesystem_is_case_sensitive;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn filesystem_is_case_sensitive_when_probe_path_is_distinct() {
        let temp = TempDir::new().expect("temp dir");
        let base = temp.path().canonicalize().expect("canonicalize temp dir");
        let path = base.join("Catalog");
        fs::create_dir_all(&path).expect("mkdir");
        let probe_path = base.join("catalog");

        assert_eq!(
            filesystem_is_case_sensitive(&path),
            !probe_path.exists(),
            "case sensitivity should match whether alternate-case probe resolves to an existing path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_is_case_insensitive_when_probe_hits_same_inode() {
        let temp = TempDir::new().expect("temp dir");
        let base = temp.path().canonicalize().expect("canonicalize temp dir");
        let path = base.join("Catalog");
        fs::create_dir_all(&path).expect("mkdir");
        let probe_path = base.join("catalog");
        if !probe_path.exists() {
            symlink(&path, &probe_path).expect("symlink alternate case");
        }

        assert!(!filesystem_is_case_sensitive(&path));
    }
}
