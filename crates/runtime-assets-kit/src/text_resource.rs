use std::io;

use crate::resource_path::normalize_resource_path;
use crate::secure_fs::MAX_TEXT_RESOURCE_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextResource {
    relative_path: String,
    contents: String,
}

impl TextResource {
    pub fn new(relative_path: impl Into<String>, contents: impl Into<String>) -> io::Result<Self> {
        let relative_path = normalize_resource_path(&relative_path.into(), false)?;
        let contents = contents.into();
        validate_text_resource_contents(&relative_path, &contents)?;
        Ok(Self {
            relative_path,
            contents,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_unchecked(
        relative_path: impl Into<String>,
        contents: impl Into<String>,
    ) -> Self {
        Self {
            relative_path: relative_path.into(),
            contents: contents.into(),
        }
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    #[must_use]
    pub(crate) fn relative_path(&self) -> &str {
        &self.relative_path
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    #[must_use]
    pub(crate) fn contents(&self) -> &str {
        &self.contents
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceManifest {
    resources: Vec<TextResource>,
}

impl ResourceManifest {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(any(feature = "i18n", feature = "prompts", test))]
    #[must_use]
    pub(crate) fn resources(&self) -> &[TextResource] {
        &self.resources
    }

    #[must_use]
    pub fn with_resource(mut self, resource: TextResource) -> Self {
        self.resources.push(resource);
        self
    }

    #[cfg(test)]
    pub(crate) fn from_resources_unchecked(resources: Vec<TextResource>) -> Self {
        Self { resources }
    }
}

#[cfg(any(feature = "i18n", feature = "prompts"))]
#[must_use]
pub(crate) fn manifest_resource_paths(manifest: &ResourceManifest) -> Vec<String> {
    manifest
        .resources()
        .iter()
        .map(|resource| resource.relative_path().to_owned())
        .collect()
}

pub(crate) fn validate_text_resource_contents(
    relative_path: &str,
    contents: &str,
) -> io::Result<()> {
    let bytes = contents.len();
    if bytes > MAX_TEXT_RESOURCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "text resource file exceeds size limit ({bytes} > {MAX_TEXT_RESOURCE_BYTES} bytes): {relative_path}"
            ),
        ));
    }

    Ok(())
}
