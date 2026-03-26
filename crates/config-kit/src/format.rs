use std::fmt;
use std::path::Path;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Toml,
    Yaml,
}

impl ConfigFormat {
    pub fn detect_opt(path: &Path) -> Result<Option<Self>> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("json") => Ok(Some(Self::Json)),
            Some(ext) if ext.eq_ignore_ascii_case("toml") => Ok(Some(Self::Toml)),
            Some(ext) if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") => {
                Ok(Some(Self::Yaml))
            }
            Some(ext) => Err(Error::UnsupportedFormat {
                path: path.to_path_buf(),
                message: format!("expected .json, .toml, .yaml, or .yml (got .{ext})"),
            }),
            None => Ok(None),
        }
    }

    pub fn parse<T>(self, raw: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.parse_with_path(raw, None)
    }

    pub fn parse_with_path<T>(self, raw: &str, path: Option<&Path>) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let result = match self {
            Self::Json => serde_json::from_str(raw).map_err(|err| err.to_string()),
            Self::Toml => toml::from_str(raw).map_err(|err| err.to_string()),
            Self::Yaml => serde_yaml::from_str(raw).map_err(|err| err.to_string()),
        };
        result.map_err(|message| Error::Parse {
            format: self,
            location: display_location(path),
            message,
        })
    }

    pub fn parse_value(self, raw: &str) -> Result<Value> {
        self.parse_value_with_path(raw, None)
    }

    pub fn parse_value_with_path(self, raw: &str, path: Option<&Path>) -> Result<Value> {
        match self {
            Self::Json => self.parse_with_path(raw, path),
            Self::Yaml => self.parse_with_path(raw, path),
            Self::Toml => {
                let value: toml::Value = self.parse_with_path(raw, path)?;
                serde_json::to_value(value).map_err(|err| Error::Parse {
                    format: self,
                    location: display_location(path),
                    message: err.to_string(),
                })
            }
        }
    }

    pub fn render<T>(self, value: &T) -> Result<String>
    where
        T: Serialize + ?Sized,
    {
        let rendered = match self {
            Self::Json => serde_json::to_string_pretty(value).map_err(|err| err.to_string()),
            Self::Toml => toml::to_string_pretty(value).map_err(|err| err.to_string()),
            Self::Yaml => serde_yaml::to_string(value).map_err(|err| err.to_string()),
        };
        rendered.map_err(|message| Error::Serialize {
            format: self,
            message,
        })
    }
}

impl fmt::Display for ConfigFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json => f.write_str("json"),
            Self::Toml => f.write_str("toml"),
            Self::Yaml => f.write_str("yaml"),
        }
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
    use serde::Serialize;

    use super::*;

    #[test]
    fn detects_supported_extensions() {
        assert_eq!(
            ConfigFormat::detect_opt(Path::new("config.json")).expect("json"),
            Some(ConfigFormat::Json)
        );
        assert_eq!(
            ConfigFormat::detect_opt(Path::new("config.toml")).expect("toml"),
            Some(ConfigFormat::Toml)
        );
        assert_eq!(
            ConfigFormat::detect_opt(Path::new("config.yaml")).expect("yaml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::detect_opt(Path::new("config.yml")).expect("yml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::detect_opt(Path::new("config")).expect("none"),
            None
        );
    }

    #[test]
    fn rejects_unknown_extensions() {
        let err = ConfigFormat::detect_opt(Path::new("config.ini")).expect_err("reject ini");
        assert!(
            err.to_string()
                .contains("expected .json, .toml, .yaml, or .yml")
        );
    }

    #[test]
    fn parses_toml_value_as_json_value() {
        let value = ConfigFormat::Toml
            .parse_value(
                r#"
                [server]
                enabled = true
                "#,
            )
            .expect("parse toml");
        assert_eq!(value["server"]["enabled"], Value::Bool(true));
    }

    #[test]
    fn renders_all_supported_formats() {
        #[derive(Serialize)]
        struct Sample<'a> {
            name: &'a str,
        }

        let sample = Sample { name: "ok" };
        assert!(
            ConfigFormat::Json
                .render(&sample)
                .expect("json")
                .contains("\"name\"")
        );
        assert!(
            ConfigFormat::Toml
                .render(&sample)
                .expect("toml")
                .contains("name = \"ok\"")
        );
        assert!(
            ConfigFormat::Yaml
                .render(&sample)
                .expect("yaml")
                .contains("name: ok")
        );
    }
}
