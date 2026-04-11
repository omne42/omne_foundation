use std::panic::{self, UnwindSafe};

use schemars::{JsonSchema, r#gen::SchemaSettings};
use ts_rs::TS;

use crate::{
    Decision, ExecutionIsolation, PolicyMetaV1, PolicyProfileV1, RiskProfile, SpecVersion,
    WriteScope,
};

pub const POLICY_META_SCHEMA_FILE: &str = "policy-meta.v1.json";
pub const POLICY_PROFILE_SCHEMA_FILE: &str = "policy-profile.v1.json";
pub const POLICY_META_TYPES_FILE: &str = "policy-meta.d.ts";
pub const POLICY_META_SCHEMA_ID: &str = "https://omne42.dev/schema/policy-meta.v1.json";
pub const POLICY_PROFILE_SCHEMA_ID: &str = "https://omne42.dev/schema/policy-profile.v1.json";
pub const POLICY_META_SCHEMA_URI: &str = "https://json-schema.org/draft/2019-09/schema";
pub const POLICY_META_SCHEMA_DESCRIPTION: &str = "Canonical policy metadata fragment. Canonical fields are reusable across repositories; field presence requirements are defined by embedding contracts. Optional version metadata may be included by persisted artifacts and preset files.";
pub const POLICY_PROFILE_SCHEMA_DESCRIPTION: &str =
    "Versioned preset profile built on top of the canonical policy metadata fragment.";

#[derive(Debug, thiserror::Error)]
pub enum ArtifactGenerationError {
    #[error("failed to serialize generated {context}: {source}")]
    Serialize {
        context: &'static str,
        source: serde_json::Error,
    },
    #[error("generated {context} must be a json object")]
    RootNotObject { context: &'static str },
    #[error("failed to generate {context}: {details}")]
    Panic { context: String, details: String },
}

pub fn policy_meta_schema_document() -> Result<serde_json::Value, ArtifactGenerationError> {
    export_schema_document::<PolicyMetaV1>(POLICY_META_SCHEMA_ID, POLICY_META_SCHEMA_DESCRIPTION)
}

pub fn policy_profile_schema_document() -> Result<serde_json::Value, ArtifactGenerationError> {
    let mut value = policy_meta_schema_document()?;
    let object = value
        .as_object_mut()
        .ok_or(ArtifactGenerationError::RootNotObject {
            context: "policy profile schema document",
        })?;

    object.insert(
        "$id".to_string(),
        serde_json::Value::String(POLICY_PROFILE_SCHEMA_ID.to_string()),
    );
    object.insert(
        "title".to_string(),
        serde_json::Value::String(PolicyProfileV1::schema_name()),
    );
    object.insert(
        "description".to_string(),
        serde_json::Value::String(POLICY_PROFILE_SCHEMA_DESCRIPTION.to_string()),
    );
    object.insert(
        "required".to_string(),
        serde_json::json!([
            "execution_isolation",
            "risk_profile",
            "version",
            "write_scope"
        ]),
    );

    Ok(value)
}

pub fn schema_documents() -> Result<[(&'static str, serde_json::Value); 2], ArtifactGenerationError>
{
    Ok([
        (POLICY_META_SCHEMA_FILE, policy_meta_schema_document()?),
        (
            POLICY_PROFILE_SCHEMA_FILE,
            policy_profile_schema_document()?,
        ),
    ])
}

pub fn profile_documents() -> [(&'static str, PolicyProfileV1); 4] {
    [
        (
            "safe.yaml",
            PolicyProfileV1::new(
                RiskProfile::Safe,
                WriteScope::ReadOnly,
                ExecutionIsolation::Strict,
            ),
        ),
        (
            "standard.yaml",
            PolicyProfileV1::new(
                RiskProfile::Standard,
                WriteScope::WorkspaceWrite,
                ExecutionIsolation::BestEffort,
            ),
        ),
        (
            "proactive.yaml",
            PolicyProfileV1::new(
                RiskProfile::Proactive,
                WriteScope::WorkspaceWrite,
                ExecutionIsolation::BestEffort,
            ),
        ),
        (
            "danger.yaml",
            PolicyProfileV1::new(
                RiskProfile::Danger,
                WriteScope::FullAccess,
                ExecutionIsolation::None,
            ),
        ),
    ]
}

pub fn policy_meta_typescript_bindings() -> Result<String, ArtifactGenerationError> {
    let declarations = [
        typescript_declaration::<SpecVersion>("SpecVersion")?,
        typescript_declaration::<RiskProfile>("RiskProfile")?,
        typescript_declaration::<WriteScope>("WriteScope")?,
        typescript_declaration::<ExecutionIsolation>("ExecutionIsolation")?,
        typescript_declaration::<Decision>("Decision")?,
        typescript_declaration::<PolicyMetaV1>("PolicyMetaV1")?,
        typescript_declaration::<PolicyProfileV1>("PolicyProfileV1")?,
    ];

    let mut output = String::from(
        "// This file was generated from crates/policy-meta by ts-rs-backed export code. Do not edit manually.\n",
    );
    for declaration in declarations {
        output.push('\n');
        output.push_str("export ");
        output.push_str(&declaration);
        output.push('\n');
    }
    Ok(output)
}

fn export_schema_document<T: JsonSchema>(
    schema_id: &'static str,
    description: &'static str,
) -> Result<serde_json::Value, ArtifactGenerationError> {
    catch_generation(format!("{} schema document", T::schema_name()), || {
        let settings = SchemaSettings::draft2019_09().with(|settings| {
            settings.option_nullable = false;
            settings.option_add_null_type = false;
            settings.inline_subschemas = true;
        });

        let value = serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
            .map_err(|source| ArtifactGenerationError::Serialize {
                context: "json schema document",
                source,
            })?;
        decorate_schema_document(
            value,
            schema_id,
            description,
            T::schema_name(),
            "json schema document",
        )
    })
}

fn decorate_schema_document(
    mut value: serde_json::Value,
    schema_id: &'static str,
    description: &'static str,
    title: String,
    context: &'static str,
) -> Result<serde_json::Value, ArtifactGenerationError> {
    let object = value
        .as_object_mut()
        .ok_or(ArtifactGenerationError::RootNotObject { context })?;

    object.insert(
        "$schema".to_string(),
        serde_json::Value::String(POLICY_META_SCHEMA_URI.to_string()),
    );
    object.insert(
        "$id".to_string(),
        serde_json::Value::String(schema_id.to_string()),
    );
    object.insert("title".to_string(), serde_json::Value::String(title));
    object.insert(
        "description".to_string(),
        serde_json::Value::String(description.to_string()),
    );

    if matches!(
        object.get("definitions"),
        Some(serde_json::Value::Object(definitions)) if definitions.is_empty()
    ) {
        object.remove("definitions");
    }

    Ok(value)
}

fn typescript_declaration<T: TS>(
    type_name: &'static str,
) -> Result<String, ArtifactGenerationError> {
    catch_generation(format!("typescript declaration for {type_name}"), || {
        Ok(T::decl())
    })
}

fn catch_generation<T>(
    context: String,
    operation: impl FnOnce() -> Result<T, ArtifactGenerationError> + UnwindSafe,
) -> Result<T, ArtifactGenerationError> {
    match panic::catch_unwind(operation) {
        Ok(result) => result,
        Err(payload) => Err(ArtifactGenerationError::Panic {
            context,
            details: panic_payload_to_string(payload),
        }),
    }
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "non-string panic payload".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn generated_policy_meta_schema_matches_contract() {
        assert_policy_meta_schema(&policy_meta_schema_document().expect("generate policy schema"));
    }

    #[test]
    fn generated_policy_profile_schema_matches_contract() {
        assert_policy_profile_schema(
            &policy_profile_schema_document().expect("generate policy profile schema"),
        );
    }

    #[test]
    fn checked_in_policy_meta_schema_matches_contract() {
        let checked_in = checked_in_schema(POLICY_META_SCHEMA_FILE);
        assert_policy_meta_schema(&checked_in);
        assert_eq!(
            checked_in,
            policy_meta_schema_document().expect("generate policy schema")
        );
    }

    #[test]
    fn checked_in_policy_profile_schema_matches_contract() {
        let checked_in = checked_in_schema(POLICY_PROFILE_SCHEMA_FILE);
        assert_policy_profile_schema(&checked_in);
        assert_eq!(
            checked_in,
            policy_profile_schema_document().expect("generate policy profile schema")
        );
    }

    #[test]
    fn generated_typescript_bindings_contain_core_types() {
        let bindings = policy_meta_typescript_bindings().expect("generate ts bindings");
        assert!(bindings.contains("export type SpecVersion = 1;"));
        assert!(bindings.contains(
            "export type RiskProfile = \"safe\" | \"standard\" | \"proactive\" | \"danger\";"
        ));
        assert!(bindings.contains("export type PolicyMetaV1 = {"));
        assert!(bindings.contains("version?: SpecVersion"));
        assert!(bindings.contains("export type PolicyProfileV1 = {"));
    }

    #[test]
    fn checked_in_typescript_bindings_match_generated_output() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("bindings")
            .join(POLICY_META_TYPES_FILE);
        let checked_in = std::fs::read_to_string(path).expect("read typescript bindings");
        assert_eq!(
            checked_in,
            policy_meta_typescript_bindings().expect("generate ts bindings")
        );
    }

    #[test]
    fn checked_in_profiles_match_expected_presets() {
        for (file_name, expected) in profile_documents() {
            assert_eq!(checked_in_profile(file_name), expected, "{file_name}");
        }
    }

    #[test]
    fn policy_profile_schema_reuses_fragment_property_domain() {
        let fragment = policy_meta_schema_document().expect("generate policy schema");
        let profile = policy_profile_schema_document().expect("generate profile schema");

        assert_eq!(profile["properties"], fragment["properties"]);
    }

    #[test]
    fn schema_decoration_rejects_non_object_root() {
        let err = decorate_schema_document(
            json!(null),
            POLICY_META_SCHEMA_ID,
            POLICY_META_SCHEMA_DESCRIPTION,
            "PolicyMetaV1".to_string(),
            "test schema document",
        )
        .expect_err("non-object roots should fail");

        assert!(matches!(
            err,
            ArtifactGenerationError::RootNotObject {
                context: "test schema document"
            }
        ));
    }

    #[test]
    fn generation_helpers_surface_panics_as_errors() {
        let err = catch_generation("panic test".to_string(), || {
            panic!("boom");
            #[allow(unreachable_code)]
            Ok::<(), ArtifactGenerationError>(())
        })
        .expect_err("panic should become an error");

        assert!(matches!(
            err,
            ArtifactGenerationError::Panic { ref context, ref details }
            if context == "panic test" && details == "boom"
        ));
    }

    #[test]
    fn artifact_generation_error_serialize_variant_exposes_display_and_source() {
        let source = serde_json::from_str::<serde_json::Value>("{")
            .expect_err("invalid json should produce a serde_json::Error");
        let expected_source_text = source.to_string();
        let err = ArtifactGenerationError::Serialize {
            context: "test schema document",
            source,
        };

        assert_eq!(
            err.to_string(),
            format!("failed to serialize generated test schema document: {expected_source_text}")
        );
        assert_eq!(
            err.source().map(std::string::ToString::to_string),
            Some(expected_source_text)
        );
    }

    fn checked_in_schema(file_name: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("schema")
            .join(file_name);
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read schema file"))
            .expect("parse schema file")
    }

    fn checked_in_profile(file_name: &str) -> PolicyProfileV1 {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("profiles")
            .join(file_name);
        serde_yaml::from_str(&std::fs::read_to_string(&path).expect("read profile file"))
            .expect("parse profile file")
    }

    fn assert_policy_meta_schema(schema: &Value) {
        assert_eq!(schema["$schema"], json!(POLICY_META_SCHEMA_URI));
        assert_eq!(schema["title"], json!("PolicyMetaV1"));
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_required_fields(schema, &[]);
        assert_common_properties(schema);
    }

    fn assert_policy_profile_schema(schema: &Value) {
        assert_eq!(schema["$schema"], json!(POLICY_META_SCHEMA_URI));
        assert_eq!(schema["title"], json!("PolicyProfileV1"));
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_required_fields(
            schema,
            &[
                "version",
                "risk_profile",
                "write_scope",
                "execution_isolation",
            ],
        );
        assert_common_properties(schema);
    }

    fn assert_common_properties(schema: &Value) {
        let properties = schema["properties"].as_object().expect("properties object");

        assert_eq!(properties["version"]["const"], json!(1));
        assert_eq!(
            properties["risk_profile"]["enum"],
            json!(canonical_risk_profiles())
        );
        assert_eq!(
            properties["write_scope"]["enum"],
            json!(canonical_write_scopes())
        );
        assert_eq!(
            properties["execution_isolation"]["enum"],
            json!(canonical_execution_isolations())
        );
        assert_eq!(properties["decision"]["enum"], json!(canonical_decisions()));
    }

    fn assert_required_fields(schema: &Value, expected: &[&str]) {
        let mut actual = schema
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        actual.sort_by(|left, right| {
            left.as_str()
                .expect("required field string")
                .cmp(right.as_str().expect("required field string"))
        });

        let mut expected = expected
            .iter()
            .copied()
            .map(Value::from)
            .collect::<Vec<_>>();
        expected.sort_by(|left, right| {
            left.as_str()
                .expect("required field string")
                .cmp(right.as_str().expect("required field string"))
        });

        assert_eq!(actual, expected);
    }

    fn canonical_risk_profiles() -> [&'static str; 4] {
        [
            RiskProfile::Safe.as_str(),
            RiskProfile::Standard.as_str(),
            RiskProfile::Proactive.as_str(),
            RiskProfile::Danger.as_str(),
        ]
    }

    fn canonical_write_scopes() -> [&'static str; 3] {
        [
            WriteScope::ReadOnly.as_str(),
            WriteScope::WorkspaceWrite.as_str(),
            WriteScope::FullAccess.as_str(),
        ]
    }

    fn canonical_execution_isolations() -> [&'static str; 3] {
        [
            ExecutionIsolation::None.as_str(),
            ExecutionIsolation::BestEffort.as_str(),
            ExecutionIsolation::Strict.as_str(),
        ]
    }

    fn canonical_decisions() -> [&'static str; 4] {
        [
            Decision::Allow.as_str(),
            Decision::Prompt.as_str(),
            Decision::PromptStrict.as_str(),
            Decision::Deny.as_str(),
        ]
    }
}
