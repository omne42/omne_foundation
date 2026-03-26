use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    ConfigFormat, ConfigLayer, ConfigLoadOptions, ConfigMergeStep, EnvInterpolationOptions, Error,
    MergedConfig, Result, find_config_document, interpolate_env_placeholders_with,
    merge_config_layers, try_load_config_document,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFileLayerOptions {
    load: ConfigLoadOptions,
    required: bool,
    interpolate_env: bool,
    interpolation_options: EnvInterpolationOptions,
}

impl SchemaFileLayerOptions {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            load: ConfigLoadOptions::new(),
            required: false,
            interpolate_env: false,
            interpolation_options: EnvInterpolationOptions::new(),
        }
    }

    #[must_use]
    pub const fn with_load_options(mut self, load: ConfigLoadOptions) -> Self {
        self.load = load;
        self
    }

    #[must_use]
    pub const fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    #[must_use]
    pub const fn with_env_interpolation(mut self, interpolate_env: bool) -> Self {
        self.interpolate_env = interpolate_env;
        self
    }

    #[must_use]
    pub const fn with_interpolation_options(
        mut self,
        interpolation_options: EnvInterpolationOptions,
    ) -> Self {
        self.interpolation_options = interpolation_options;
        self
    }
}

impl Default for SchemaFileLayerOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSchemaLayer {
    name: String,
    path: Option<PathBuf>,
    format: Option<ConfigFormat>,
    changed_paths: Vec<String>,
}

impl LoadedSchemaLayer {
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[must_use]
    pub const fn format(&self) -> Option<ConfigFormat> {
        self.format
    }

    #[must_use]
    pub fn changed_paths(&self) -> &[String] {
        self.changed_paths.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSchemaConfig<T> {
    value: T,
    merged_value: Value,
    merged: MergedConfig,
    layers: Vec<LoadedSchemaLayer>,
}

impl<T> LoadedSchemaConfig<T> {
    #[must_use]
    pub fn value(&self) -> &T {
        &self.value
    }

    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }

    #[must_use]
    pub fn merged_value(&self) -> &Value {
        &self.merged_value
    }

    #[must_use]
    pub fn merged(&self) -> &MergedConfig {
        &self.merged
    }

    #[must_use]
    pub fn layers(&self) -> &[LoadedSchemaLayer] {
        self.layers.as_slice()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchemaConfigLoader {
    sources: Vec<SchemaSource>,
}

#[derive(Debug, Clone)]
enum SchemaSource {
    Value {
        name: String,
        value: Value,
    },
    ExplicitFile {
        name: String,
        path: PathBuf,
        options: SchemaFileLayerOptions,
    },
    CandidateFiles {
        name: String,
        root: PathBuf,
        candidates: Vec<PathBuf>,
        options: SchemaFileLayerOptions,
    },
}

#[derive(Debug, Clone)]
struct ResolvedSchemaLayer {
    config_layer: ConfigLayer,
    loaded_layer: LoadedSchemaLayer,
}

impl SchemaConfigLoader {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    #[must_use]
    pub fn add_value_layer(mut self, name: impl Into<String>, value: Value) -> Self {
        self.sources.push(SchemaSource::Value {
            name: name.into(),
            value,
        });
        self
    }

    pub fn add_serializable_layer<T>(self, name: impl Into<String>, value: &T) -> Result<Self>
    where
        T: Serialize + ?Sized,
    {
        let value = serde_json::to_value(value).map_err(|err| Error::Serialize {
            format: ConfigFormat::Json,
            message: err.to_string(),
        })?;
        Ok(self.add_value_layer(name, value))
    }

    #[must_use]
    pub fn add_file_layer(
        mut self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        options: SchemaFileLayerOptions,
    ) -> Self {
        self.sources.push(SchemaSource::ExplicitFile {
            name: name.into(),
            path: path.into(),
            options,
        });
        self
    }

    pub fn add_candidate_file_layer<I, P>(
        mut self,
        name: impl Into<String>,
        root: impl Into<PathBuf>,
        candidates: I,
        options: SchemaFileLayerOptions,
    ) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.sources.push(SchemaSource::CandidateFiles {
            name: name.into(),
            root: root.into(),
            candidates: candidates.into_iter().map(Into::into).collect(),
            options,
        });
        self
    }

    pub fn load_optional<T>(&self) -> Result<Option<LoadedSchemaConfig<T>>>
    where
        T: DeserializeOwned,
    {
        self.load_optional_with_env_lookup(|name| std::env::var(name).ok())
    }

    pub fn load_optional_with_env_lookup<T, F>(
        &self,
        mut lookup: F,
    ) -> Result<Option<LoadedSchemaConfig<T>>>
    where
        T: DeserializeOwned,
        F: FnMut(&str) -> Option<String>,
    {
        let resolved_layers = self.resolve_layers(|name| lookup(name))?;
        if resolved_layers.is_empty() {
            return Ok(None);
        }
        Ok(Some(build_loaded_schema_config(resolved_layers)?))
    }

    pub fn load<T>(&self) -> Result<LoadedSchemaConfig<T>>
    where
        T: DeserializeOwned,
    {
        self.load_optional()?.ok_or_else(|| Error::InvalidOptions {
            message: "schema config loader resolved no layers".to_string(),
        })
    }

    pub fn load_with_env_lookup<T, F>(&self, lookup: F) -> Result<LoadedSchemaConfig<T>>
    where
        T: DeserializeOwned,
        F: FnMut(&str) -> Option<String>,
    {
        self.load_optional_with_env_lookup(lookup)?
            .ok_or_else(|| Error::InvalidOptions {
                message: "schema config loader resolved no layers".to_string(),
            })
    }

    fn resolve_layers<F>(&self, mut lookup: F) -> Result<Vec<ResolvedSchemaLayer>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut resolved = Vec::new();
        for source in &self.sources {
            if let Some(layer) = source.resolve(&mut lookup)? {
                resolved.push(layer);
            }
        }
        Ok(resolved)
    }
}

impl SchemaSource {
    fn resolve<F>(&self, lookup: &mut F) -> Result<Option<ResolvedSchemaLayer>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        match self {
            Self::Value { name, value } => Ok(Some(ResolvedSchemaLayer {
                config_layer: ConfigLayer::new(name.clone(), value.clone()),
                loaded_layer: LoadedSchemaLayer {
                    name: name.clone(),
                    path: None,
                    format: None,
                    changed_paths: Vec::new(),
                },
            })),
            Self::ExplicitFile {
                name,
                path,
                options,
            } => resolve_explicit_file_layer(name, path, options, lookup),
            Self::CandidateFiles {
                name,
                root,
                candidates,
                options,
            } => resolve_candidate_file_layer(name, root, candidates, options, lookup),
        }
    }
}

fn resolve_explicit_file_layer<F>(
    name: &str,
    path: &Path,
    options: &SchemaFileLayerOptions,
    lookup: &mut F,
) -> Result<Option<ResolvedSchemaLayer>>
where
    F: FnMut(&str) -> Option<String>,
{
    let Some(document) = try_load_config_document(path, options.load)? else {
        if options.required {
            return Err(Error::RequiredLayerMissing {
                name: name.to_string(),
                location: format!(" at {}", path.display()),
            });
        }
        return Ok(None);
    };

    Ok(Some(resolved_layer_from_document(
        name, document, options, lookup,
    )?))
}

fn resolve_candidate_file_layer<F>(
    name: &str,
    root: &Path,
    candidates: &[PathBuf],
    options: &SchemaFileLayerOptions,
    lookup: &mut F,
) -> Result<Option<ResolvedSchemaLayer>>
where
    F: FnMut(&str) -> Option<String>,
{
    let Some(document) = find_config_document(root, candidates.iter(), options.load)? else {
        if options.required {
            let joined = candidates
                .iter()
                .map(|candidate| candidate.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::RequiredLayerMissing {
                name: name.to_string(),
                location: format!(" under {} (tried: {joined})", root.display()),
            });
        }
        return Ok(None);
    };

    Ok(Some(resolved_layer_from_document(
        name, document, options, lookup,
    )?))
}

fn resolved_layer_from_document<F>(
    name: &str,
    document: crate::ConfigDocument,
    options: &SchemaFileLayerOptions,
    lookup: &mut F,
) -> Result<ResolvedSchemaLayer>
where
    F: FnMut(&str) -> Option<String>,
{
    let value = if options.interpolate_env {
        let interpolated = interpolate_env_placeholders_with(
            document.contents(),
            options.interpolation_options,
            lookup,
        )?;
        document
            .format()
            .parse_value_with_path(&interpolated, Some(document.path()))?
    } else {
        document.parse_value()?
    };

    Ok(ResolvedSchemaLayer {
        config_layer: ConfigLayer::new(name.to_string(), value),
        loaded_layer: LoadedSchemaLayer {
            name: name.to_string(),
            path: Some(document.path().to_path_buf()),
            format: Some(document.format()),
            changed_paths: Vec::new(),
        },
    })
}

fn build_loaded_schema_config<T>(
    resolved_layers: Vec<ResolvedSchemaLayer>,
) -> Result<LoadedSchemaConfig<T>>
where
    T: DeserializeOwned,
{
    let merge_inputs = resolved_layers
        .iter()
        .map(|layer| layer.config_layer.clone())
        .collect::<Vec<_>>();
    let merged = merge_config_layers(merge_inputs);
    let merged_value = merged.value().clone();
    let value = serde_json::from_value(merged_value.clone()).map_err(|err| Error::Parse {
        format: ConfigFormat::Json,
        location: String::new(),
        message: err.to_string(),
    })?;
    let layers = resolved_layers
        .into_iter()
        .zip(merged.steps().iter())
        .map(map_loaded_schema_layer)
        .collect();

    Ok(LoadedSchemaConfig {
        value,
        merged_value,
        merged,
        layers,
    })
}

fn map_loaded_schema_layer(
    (resolved, step): (ResolvedSchemaLayer, &ConfigMergeStep),
) -> LoadedSchemaLayer {
    let mut layer = resolved.loaded_layer;
    layer.changed_paths = step.changed_paths().to_vec();
    layer
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct SampleConfig {
        enabled: bool,
        #[serde(default)]
        name: Option<String>,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct LayeredServiceConfig {
        enabled: bool,
        service: LayeredServiceSection,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct LayeredServiceSection {
        base_url: String,
        timeout_ms: u64,
    }

    #[derive(Debug, Serialize)]
    struct LayeredDefaults<'a> {
        enabled: bool,
        service: LayeredDefaultsService<'a>,
    }

    #[derive(Debug, Serialize)]
    struct LayeredDefaultsService<'a> {
        base_url: &'a str,
        timeout_ms: u64,
    }

    #[test]
    fn load_optional_returns_none_when_all_optional_sources_are_missing() {
        let dir = tempfile::tempdir().expect("dir");
        let loader = SchemaConfigLoader::new().add_candidate_file_layer(
            "project",
            dir.path(),
            ["config.toml"],
            SchemaFileLayerOptions::new(),
        );
        let loaded = loader.load_optional::<SampleConfig>().expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_rejects_missing_required_candidate_layer() {
        let dir = tempfile::tempdir().expect("dir");
        let loader = SchemaConfigLoader::new().add_candidate_file_layer(
            "project",
            dir.path(),
            ["config.toml"],
            SchemaFileLayerOptions::new().required(true),
        );
        let err = loader
            .load_optional::<SampleConfig>()
            .expect_err("required");
        assert!(err.to_string().contains("required config layer"));
    }

    #[test]
    fn load_rejects_missing_required_explicit_layer() {
        let dir = tempfile::tempdir().expect("dir");
        let missing = dir.path().join("missing.toml");
        let loader = SchemaConfigLoader::new().add_file_layer(
            "project",
            &missing,
            SchemaFileLayerOptions::new().required(true),
        );
        let err = loader
            .load_optional::<SampleConfig>()
            .expect_err("required explicit file");
        assert_eq!(
            err.to_string(),
            format!(
                "required config layer project not found at {}",
                missing.display()
            )
        );
    }

    #[test]
    fn load_merges_layers_and_reports_changed_paths() {
        let loader = SchemaConfigLoader::new()
            .add_value_layer("defaults", json!({"enabled": false}))
            .add_value_layer("env", json!({"enabled": true, "name": "demo"}));

        let loaded = loader.load::<SampleConfig>().expect("load");
        assert_eq!(
            loaded.value(),
            &SampleConfig {
                enabled: true,
                name: Some("demo".to_string()),
            }
        );
        assert_eq!(loaded.layers()[0].changed_paths(), ["/enabled"]);
        assert_eq!(loaded.layers()[1].changed_paths(), ["/enabled", "/name"]);
    }

    #[test]
    fn load_supports_env_interpolation_with_custom_lookup() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"enabled": ${FLAG}, "name": "${NAME}"}"#).expect("write");

        let loader = SchemaConfigLoader::new().add_file_layer(
            "file",
            &path,
            SchemaFileLayerOptions::new().with_env_interpolation(true),
        );

        let loaded = loader
            .load_with_env_lookup::<SampleConfig, _>(|key| match key {
                "FLAG" => Some("true".to_string()),
                "NAME" => Some("demo".to_string()),
                _ => None,
            })
            .expect("load");

        assert_eq!(
            loaded.value(),
            &SampleConfig {
                enabled: true,
                name: Some("demo".to_string()),
            }
        );
        assert_eq!(loaded.layers()[0].path(), Some(path.as_path()));
        assert_eq!(loaded.layers()[0].format(), Some(ConfigFormat::Json));
    }

    #[test]
    fn load_supports_layered_schema_pattern_for_project_configs() {
        let dir = tempfile::tempdir().expect("dir");
        let omne_data = dir.path().join(".omne_data");
        std::fs::create_dir_all(&omne_data).expect("create omne_data");

        let shared = omne_data.join("config.toml");
        std::fs::write(
            &shared,
            concat!(
                "enabled = ${ENABLED}\n",
                "\n",
                "[service]\n",
                "base_url = \"${BASE_URL}\"\n",
            ),
        )
        .expect("write shared config");

        let defaults = LayeredDefaults {
            enabled: false,
            service: LayeredDefaultsService {
                base_url: "https://localhost:8080",
                timeout_ms: 30_000,
            },
        };

        let loaded = SchemaConfigLoader::new()
            .add_serializable_layer("defaults", &defaults)
            .expect("defaults")
            .add_candidate_file_layer(
                "project",
                dir.path(),
                [".omne_data/config_local.toml", ".omne_data/config.toml"],
                SchemaFileLayerOptions::new().with_env_interpolation(true),
            )
            .load_with_env_lookup::<LayeredServiceConfig, _>(|name| match name {
                "ENABLED" => Some("true".to_string()),
                "BASE_URL" => Some("https://api.example.com".to_string()),
                _ => None,
            })
            .expect("load layered config");

        assert_eq!(
            loaded.value(),
            &LayeredServiceConfig {
                enabled: true,
                service: LayeredServiceSection {
                    base_url: "https://api.example.com".to_string(),
                    timeout_ms: 30_000,
                },
            }
        );
        assert_eq!(loaded.layers().len(), 2);
        assert_eq!(loaded.layers()[0].name(), "defaults");
        assert_eq!(loaded.layers()[0].changed_paths(), ["/enabled", "/service"]);
        assert_eq!(loaded.layers()[1].name(), "project");
        assert_eq!(loaded.layers()[1].path(), Some(shared.as_path()));
        assert_eq!(loaded.layers()[1].format(), Some(ConfigFormat::Toml));
        assert_eq!(
            loaded.layers()[1].changed_paths(),
            ["/enabled", "/service/base_url"]
        );
    }

    #[test]
    fn load_exposes_merged_value_for_raw_consumers() {
        let loaded = SchemaConfigLoader::new()
            .add_value_layer("defaults", json!({"enabled": true}))
            .load::<Value>()
            .expect("load");
        assert_eq!(loaded.merged_value(), &json!({"enabled": true}));
    }
}
