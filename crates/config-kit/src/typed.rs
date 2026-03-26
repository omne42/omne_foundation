use std::fmt;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::{
    ConfigDocument, ConfigFormat, ConfigLoadOptions, Error, Result, load_config_document,
    try_load_config_document,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigFormatSet(u8);

impl ConfigFormatSet {
    pub const NONE: Self = Self(0);
    pub const JSON: Self = Self(bit(ConfigFormat::Json));
    pub const TOML: Self = Self(bit(ConfigFormat::Toml));
    pub const YAML: Self = Self(bit(ConfigFormat::Yaml));
    pub const JSON_TOML: Self = Self(Self::JSON.0 | Self::TOML.0);
    pub const JSON_YAML: Self = Self(Self::JSON.0 | Self::YAML.0);
    pub const TOML_YAML: Self = Self(Self::TOML.0 | Self::YAML.0);
    pub const ALL: Self = Self(Self::JSON.0 | Self::TOML.0 | Self::YAML.0);

    #[must_use]
    pub const fn from_format(format: ConfigFormat) -> Self {
        Self(bit(format))
    }

    #[must_use]
    pub const fn contains(self, format: ConfigFormat) -> bool {
        self.0 & bit(format) != 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self.0 {
            0 => "no formats",
            bits if bits == Self::JSON.0 => "json",
            bits if bits == Self::TOML.0 => "toml",
            bits if bits == Self::YAML.0 => "yaml",
            bits if bits == Self::JSON_TOML.0 => "json or toml",
            bits if bits == Self::JSON_YAML.0 => "json or yaml",
            bits if bits == Self::TOML_YAML.0 => "toml or yaml",
            bits if bits == Self::ALL.0 => "json, toml, or yaml",
            _ => "unknown formats",
        }
    }
}

impl fmt::Display for ConfigFormatSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

impl ConfigDocument {
    pub fn parse_as<T>(&self, allowed_formats: ConfigFormatSet) -> Result<T>
    where
        T: DeserializeOwned,
    {
        ensure_allowed_format(self.format(), Some(self.path()), allowed_formats)?;
        self.parse()
    }
}

pub fn parse_typed_config_document<T>(
    document: &ConfigDocument,
    allowed_formats: ConfigFormatSet,
) -> Result<T>
where
    T: DeserializeOwned,
{
    document.parse_as(allowed_formats)
}

pub fn load_typed_config_file<T>(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
    allowed_formats: ConfigFormatSet,
) -> Result<T>
where
    T: DeserializeOwned,
{
    let document = load_config_document(path, options)?;
    document.parse_as(allowed_formats)
}

pub fn try_load_typed_config_file<T>(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
    allowed_formats: ConfigFormatSet,
) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    let Some(document) = try_load_config_document(path, options)? else {
        return Ok(None);
    };
    document.parse_as(allowed_formats).map(Some)
}

fn ensure_allowed_format(
    format: ConfigFormat,
    path: Option<&Path>,
    allowed_formats: ConfigFormatSet,
) -> Result<()> {
    if allowed_formats.is_empty() {
        return Err(Error::InvalidOptions {
            message: "allowed_formats must not be empty".to_string(),
        });
    }
    if allowed_formats.contains(format) {
        return Ok(());
    }
    Err(Error::FormatNotAllowed {
        format,
        location: display_location(path),
        expected: allowed_formats.to_string(),
    })
}

const fn bit(format: ConfigFormat) -> u8 {
    match format {
        ConfigFormat::Json => 1,
        ConfigFormat::Toml => 1 << 1,
        ConfigFormat::Yaml => 1 << 2,
    }
}

fn display_location(path: Option<&Path>) -> String {
    match path {
        Some(path) => format!(" at {}", path.display()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct SampleConfig {
        enabled: bool,
    }

    #[test]
    fn parse_document_accepts_allowed_format() {
        let document = ConfigDocument::new(
            "config.yaml".into(),
            ConfigFormat::Yaml,
            "enabled: true\n".to_string(),
        );

        let parsed: SampleConfig = document
            .parse_as(ConfigFormatSet::JSON_YAML)
            .expect("parse allowed yaml");
        assert_eq!(parsed, SampleConfig { enabled: true });
    }

    #[test]
    fn parse_document_rejects_disallowed_format() {
        let document = ConfigDocument::new(
            "config.toml".into(),
            ConfigFormat::Toml,
            "enabled = true\n".to_string(),
        );

        let err = document
            .parse_as::<SampleConfig>(ConfigFormatSet::JSON_YAML)
            .expect_err("reject toml");
        assert_eq!(
            err.to_string(),
            "config format toml is not allowed at config.toml: expected json or yaml"
        );
    }

    #[test]
    fn load_typed_config_file_uses_document_format_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sample.json");
        std::fs::write(&path, "{ \"enabled\": true }\n").expect("write");

        let parsed: SampleConfig =
            load_typed_config_file(&path, ConfigLoadOptions::new(), ConfigFormatSet::JSON)
                .expect("load typed");
        assert_eq!(parsed, SampleConfig { enabled: true });
    }

    #[test]
    fn try_load_typed_config_file_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.yaml");

        let parsed = try_load_typed_config_file::<SampleConfig>(
            &path,
            ConfigLoadOptions::new(),
            ConfigFormatSet::YAML,
        )
        .expect("missing config should not fail");
        assert!(parsed.is_none());
    }
}
