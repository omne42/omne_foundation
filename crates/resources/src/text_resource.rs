use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextResource {
    pub relative_path: String,
    pub contents: String,
}

impl TextResource {
    #[must_use]
    pub fn new(relative_path: impl Into<String>, contents: impl Into<String>) -> Self {
        Self {
            relative_path: relative_path.into(),
            contents: contents.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceManifest {
    pub name: String,
    pub directories: Vec<String>,
    pub resources: Vec<TextResource>,
}

impl ResourceManifest {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            directories: Vec::new(),
            resources: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_directory(mut self, relative_path: impl Into<String>) -> Self {
        self.directories.push(relative_path.into());
        self
    }

    #[must_use]
    pub fn with_resource(mut self, resource: TextResource) -> Self {
        self.resources.push(resource);
        self
    }
}

#[derive(Debug, Default)]
pub struct BootstrapReport {
    pub created: Vec<std::path::PathBuf>,
    pub existing: Vec<std::path::PathBuf>,
}

pub fn bootstrap_text_resources(
    root: &Path,
    manifest: &ResourceManifest,
) -> io::Result<BootstrapReport> {
    fs::create_dir_all(root)?;

    for directory in &manifest.directories {
        if directory.is_empty() {
            continue;
        }
        fs::create_dir_all(root.join(directory))?;
    }

    let mut report = BootstrapReport::default();
    for resource in &manifest.resources {
        let target = root.join(&resource.relative_path);
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        if target.exists() {
            report.existing.push(target);
            continue;
        }

        fs::write(&target, &resource.contents)?;
        report.created.push(target);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bootstrap_creates_directories_and_resources() {
        let temp = TempDir::new().expect("temp dir");

        let manifest = ResourceManifest::new("test")
            .with_directory("i18n")
            .with_resource(TextResource::new(
                "i18n/en_US.json",
                "{\"hello\":\"world\"}",
            ))
            .with_resource(TextResource::new("prompts/default.md", "hi"));

        let first = bootstrap_text_resources(temp.path(), &manifest).expect("first bootstrap");
        assert_eq!(first.created.len(), 2);
        assert!(temp.path().join("i18n").exists());
        assert!(temp.path().join("prompts/default.md").exists());

        let second = bootstrap_text_resources(temp.path(), &manifest).expect("second bootstrap");
        assert!(second.created.is_empty());
        assert_eq!(second.existing.len(), 2);
    }
}
