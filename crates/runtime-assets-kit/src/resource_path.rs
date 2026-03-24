use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn materialize_resource_root(root: &Path) -> io::Result<PathBuf> {
    omne_systems_fs_primitives::materialize_root(root, "resource root")
}

pub(crate) fn normalize_resource_path(
    relative_path: &str,
    allow_empty: bool,
) -> io::Result<String> {
    let components = relative_resource_components(relative_path, allow_empty)?;
    if components.is_empty() {
        return Ok(String::new());
    }
    Ok(components.join("/"))
}

pub(crate) fn resource_identity_key(relative_path: &str, allow_empty: bool) -> io::Result<String> {
    let components = relative_resource_components(relative_path, allow_empty)?;
    Ok(join_resource_identity_components(&components))
}

fn join_resource_identity_components(components: &[&str]) -> String {
    components
        .iter()
        .map(|component| portable_resource_component_key(component))
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn relative_resource_components(
    relative_path: &str,
    allow_empty: bool,
) -> io::Result<Vec<&str>> {
    if relative_path.is_empty() {
        return if allow_empty {
            Ok(Vec::new())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "resource path cannot be empty",
            ))
        };
    }

    if relative_path.starts_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path must stay within root: {relative_path}"),
        ));
    }

    let mut components = Vec::new();
    for component in relative_path.split('/') {
        if component.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("resource path must not contain empty components: {relative_path}"),
            ));
        }

        validate_relative_resource_component(component, relative_path)?;
        components.push(component);
    }

    if components.is_empty() {
        return if allow_empty {
            Ok(Vec::new())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "resource path cannot be empty",
            ))
        };
    }

    Ok(components)
}

pub(crate) fn validate_relative_resource_component(
    component: &str,
    relative_path: &str,
) -> io::Result<()> {
    if matches!(component, "." | "..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path must stay within root: {relative_path}"),
        ));
    }

    if component.contains('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path component contains backslash: {relative_path}"),
        ));
    }

    if component.contains(':') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path component contains colon: {relative_path}"),
        ));
    }

    if component.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path component contains NUL byte: {relative_path:?}"),
        ));
    }

    if component
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "resource path component contains Windows-reserved characters: {relative_path}"
            ),
        ));
    }

    if component.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path component contains control characters: {relative_path:?}"),
        ));
    }

    if component.ends_with(' ') || component.ends_with('.') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path component must not end with a space or dot: {relative_path}"),
        ));
    }

    if windows_reserved_component_name(component) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path component uses a Windows-reserved device name: {relative_path}"),
        ));
    }

    Ok(())
}

fn portable_resource_component_key(component: &str) -> String {
    component.to_lowercase()
}

fn windows_reserved_component_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::secure_fs::MAX_TEXT_RESOURCE_BYTES;
    use crate::text_resource::TextResource;

    #[test]
    fn text_resource_rejects_empty_components() {
        let error = TextResource::new("nested//system.md", "hello")
            .expect_err("empty path component should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("empty components"));

        let error = TextResource::new("nested/system.md/", "hello")
            .expect_err("trailing slash should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("empty components"));
    }

    #[test]
    fn text_resource_rejects_absolute_like_paths() {
        let error = TextResource::new("/nested/system.md", "hello")
            .expect_err("absolute resource path should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("stay within root"));
    }

    #[test]
    fn text_resource_rejects_windows_drive_like_paths() {
        let error = TextResource::new("C:/nested/system.md", "hello")
            .expect_err("drive-like resource path should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("contains colon"));
    }

    #[test]
    fn text_resource_rejects_windows_reserved_characters() {
        let error = TextResource::new(r#"nested/what?.md"#, "hello")
            .expect_err("reserved char should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            error
                .to_string()
                .contains("contains Windows-reserved characters")
        );
    }

    #[test]
    fn text_resource_rejects_windows_reserved_device_names() {
        let error =
            TextResource::new("nested/CON.txt", "hello").expect_err("reserved name should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("Windows-reserved device name"));
    }

    #[test]
    fn text_resource_rejects_components_with_trailing_dots_or_spaces() {
        let trailing_dot =
            TextResource::new("nested/system.md.", "hello").expect_err("trailing dot should fail");
        assert_eq!(trailing_dot.kind(), io::ErrorKind::InvalidInput);
        assert!(
            trailing_dot
                .to_string()
                .contains("must not end with a space or dot")
        );

        let trailing_space = TextResource::new("nested/system.md ", "hello")
            .expect_err("trailing space should fail");
        assert_eq!(trailing_space.kind(), io::ErrorKind::InvalidInput);
        assert!(
            trailing_space
                .to_string()
                .contains("must not end with a space or dot")
        );
    }

    #[test]
    fn text_resource_rejects_dot_segments() {
        let error =
            TextResource::new("nested/./system.md", "hello").expect_err("dot segment should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("stay within root"));
    }

    #[test]
    fn text_resource_rejects_oversized_contents() {
        let error = TextResource::new("huge.txt", "x".repeat(MAX_TEXT_RESOURCE_BYTES + 1))
            .expect_err("oversized resource should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds size limit"));
    }

    #[test]
    fn resource_identity_key_is_case_insensitive_for_portability() {
        assert_eq!(
            resource_identity_key("nested/Prompt.md", false).expect("identity"),
            resource_identity_key("NESTED/prompt.md", false).expect("identity")
        );
    }
}
