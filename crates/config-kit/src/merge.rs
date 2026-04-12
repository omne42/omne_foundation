use serde::Serialize;
use serde_json::{Map, Value};

use crate::{ConfigDocument, Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLayer {
    name: String,
    value: Value,
}

impl ConfigLayer {
    #[must_use]
    pub fn new(name: impl Into<String>, value: Value) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }

    pub fn from_document(name: impl Into<String>, document: &ConfigDocument) -> Result<Self> {
        Ok(Self::new(name, document.parse_value()?))
    }

    pub fn from_serializable<T>(name: impl Into<String>, value: &T) -> Result<Self>
    where
        T: Serialize + ?Sized,
    {
        let value = serde_json::to_value(value).map_err(|err| Error::Serialize {
            format: crate::ConfigFormat::Json,
            message: err.to_string(),
        })?;
        Ok(Self::new(name, value))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMergeStep {
    layer_name: String,
    changed_paths: Vec<String>,
}

impl ConfigMergeStep {
    #[must_use]
    pub fn layer_name(&self) -> &str {
        self.layer_name.as_str()
    }

    #[must_use]
    pub fn changed_paths(&self) -> &[String] {
        self.changed_paths.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergedConfig {
    value: Value,
    steps: Vec<ConfigMergeStep>,
}

impl MergedConfig {
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    #[must_use]
    pub fn steps(&self) -> &[ConfigMergeStep] {
        self.steps.as_slice()
    }

    pub fn parse<T>(&self) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_value(self.value.clone()).map_err(|err| Error::Parse {
            format: crate::ConfigFormat::Json,
            location: String::new(),
            message: err.to_string(),
        })
    }
}

pub fn merge_config_layers<I>(layers: I) -> MergedConfig
where
    I: IntoIterator<Item = ConfigLayer>,
{
    let mut merged = Value::Object(Map::new());
    let mut steps = Vec::new();
    for layer in layers {
        let changed_paths = merge_config_values_in_place(&mut merged, layer.value);
        steps.push(ConfigMergeStep {
            layer_name: layer.name,
            changed_paths,
        });
    }
    MergedConfig {
        value: merged,
        steps,
    }
}

#[must_use]
pub fn merge_config_values(base: Value, overlay: Value) -> (Value, Vec<String>) {
    let mut merged = base;
    let changed_paths = merge_config_values_in_place(&mut merged, overlay);
    (merged, changed_paths)
}

#[must_use]
pub fn merge_config_values_in_place(base: &mut Value, overlay: Value) -> Vec<String> {
    let mut changed_paths = Vec::new();
    merge_value(base, overlay, String::new(), &mut changed_paths);
    changed_paths.sort();
    changed_paths
}

fn merge_value(base: &mut Value, overlay: Value, path: String, changed_paths: &mut Vec<String>) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            merge_object(base_map, overlay_map, path, changed_paths);
        }
        (base_slot, overlay_value) => {
            if *base_slot != overlay_value {
                *base_slot = overlay_value;
                changed_paths.push(json_pointer_path(path));
            }
        }
    }
}

fn merge_object(
    base_map: &mut Map<String, Value>,
    overlay_map: Map<String, Value>,
    parent_path: String,
    changed_paths: &mut Vec<String>,
) {
    for (key, overlay_value) in overlay_map {
        let child_path = push_path(&parent_path, &key);
        match base_map.get_mut(&key) {
            Some(base_value) => merge_value(base_value, overlay_value, child_path, changed_paths),
            None => {
                base_map.insert(key, overlay_value);
                changed_paths.push(child_path);
            }
        }
    }
}

fn push_path(parent_path: &str, key: &str) -> String {
    let escaped = key.replace('~', "~0").replace('/', "~1");
    if parent_path.is_empty() {
        format!("/{escaped}")
    } else {
        format!("{parent_path}/{escaped}")
    }
}

fn json_pointer_path(path: String) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn merge_recursively_overlays_objects() {
        let (merged, changed_paths) = merge_config_values(
            json!({
                "http": { "timeout": 30, "headers": { "x-a": "1" } },
                "enabled": true
            }),
            json!({
                "http": { "headers": { "x-b": "2" } },
                "enabled": false
            }),
        );

        assert_eq!(
            merged,
            json!({
                "http": { "timeout": 30, "headers": { "x-a": "1", "x-b": "2" } },
                "enabled": false
            })
        );
        assert_eq!(changed_paths, vec!["/enabled", "/http/headers/x-b"]);
    }

    #[test]
    fn merge_replaces_non_object_branches() {
        let (merged, changed_paths) =
            merge_config_values(json!({"http": true}), json!({"http": {"timeout": 30}}));
        assert_eq!(merged, json!({"http": {"timeout": 30}}));
        assert_eq!(changed_paths, vec!["/http"]);
    }

    #[test]
    fn merge_replacing_root_uses_json_pointer_root_path() {
        let (merged, changed_paths) = merge_config_values(json!({"http": true}), json!(false));
        assert_eq!(merged, json!(false));
        assert_eq!(changed_paths, vec!["/"]);
    }

    #[test]
    fn merge_layers_records_each_step() {
        let merged = merge_config_layers([
            ConfigLayer::new("defaults", json!({"http": {"timeout": 30}})),
            ConfigLayer::new(
                "file",
                json!({"http": {"timeout": 60, "headers": {"x": "1"}}}),
            ),
            ConfigLayer::new("env", json!({"http": {"headers": {"y": "2"}}})),
        ]);

        assert_eq!(
            merged.value(),
            &json!({"http": {"timeout": 60, "headers": {"x": "1", "y": "2"}}})
        );
        assert_eq!(merged.steps()[0].changed_paths(), ["/http"]);
        assert_eq!(
            merged.steps()[1].changed_paths(),
            ["/http/headers", "/http/timeout"]
        );
        assert_eq!(merged.steps()[2].changed_paths(), ["/http/headers/y"]);
    }
}
