use std::{borrow::Cow, collections::BTreeSet, error::Error, fs, path::Path};

use schemars::{
    JsonSchema,
    r#gen::{SchemaGenerator, SchemaSettings},
    schema::{InstanceType, Schema, SchemaObject, SingleOrVec},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as DeError, Unexpected},
};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RiskProfile {
    Safe,
    Standard,
    Proactive,
    Danger,
}

impl RiskProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Standard => "standard",
            Self::Proactive => "proactive",
            Self::Danger => "danger",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteScope {
    #[default]
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl WriteScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceWrite => "workspace_write",
            Self::FullAccess => "full_access",
        }
    }

    pub const fn allows_write(self) -> bool {
        matches!(self, Self::WorkspaceWrite | Self::FullAccess)
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionIsolation {
    None,
    BestEffort,
    Strict,
}

impl ExecutionIsolation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::BestEffort => "best_effort",
            Self::Strict => "strict",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Prompt,
    PromptStrict,
    Deny,
}

impl Decision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Prompt => "prompt",
            Self::PromptStrict => "prompt_strict",
            Self::Deny => "deny",
        }
    }
}

pub const POLICY_META_SPEC_VERSION: u8 = 1;
pub const POLICY_META_SCHEMA_FILE: &str = "policy-meta.v1.json";
pub const POLICY_PROFILE_SCHEMA_FILE: &str = "policy-profile.v1.json";
pub const POLICY_META_TYPES_FILE: &str = "policy-meta.d.ts";
pub const POLICY_META_SCHEMA_ID: &str = "https://omne42.dev/schema/policy-meta.v1.json";
pub const POLICY_PROFILE_SCHEMA_ID: &str = "https://omne42.dev/schema/policy-profile.v1.json";
pub const POLICY_META_SCHEMA_URI: &str = "https://json-schema.org/draft/2020-12/schema";
pub const POLICY_META_SCHEMA_DESCRIPTION: &str = "Canonical policy metadata fragment. Canonical fields are reusable across repositories; field presence requirements are defined by embedding contracts. Optional version metadata may be included by persisted artifacts and preset files.";
pub const POLICY_PROFILE_SCHEMA_DESCRIPTION: &str =
    "Versioned preset profile built on top of the canonical policy metadata fragment.";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, TS)]
#[ts(type = "1")]
pub struct SpecVersion;

impl SpecVersion {
    pub const V1: Self = Self;

    pub const fn as_u8(self) -> u8 {
        POLICY_META_SPEC_VERSION
    }
}

impl Serialize for SpecVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(POLICY_META_SPEC_VERSION)
    }
}

impl<'de> Deserialize<'de> for SpecVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        if value == POLICY_META_SPEC_VERSION {
            Ok(Self)
        } else {
            Err(D::Error::invalid_value(
                Unexpected::Unsigned(u64::from(value)),
                &"integer 1",
            ))
        }
    }
}

impl JsonSchema for SpecVersion {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        "SpecVersion".to_owned()
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed(concat!(module_path!(), "::SpecVersion"))
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        Schema::Object(SchemaObject {
            instance_type: Some(SingleOrVec::Single(Box::new(InstanceType::Integer))),
            const_value: Some(serde_json::Value::from(POLICY_META_SPEC_VERSION)),
            ..Default::default()
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PolicyMetaV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<SpecVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub risk_profile: Option<RiskProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub write_scope: Option<WriteScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub execution_isolation: Option<ExecutionIsolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decision: Option<Decision>,
}

impl PolicyMetaV1 {
    pub const fn new() -> Self {
        Self {
            version: None,
            risk_profile: None,
            write_scope: None,
            execution_isolation: None,
            decision: None,
        }
    }

    pub const fn with_version(mut self) -> Self {
        self.version = Some(SpecVersion::V1);
        self
    }

    pub const fn with_risk_profile(mut self, risk_profile: RiskProfile) -> Self {
        self.risk_profile = Some(risk_profile);
        self
    }

    pub const fn with_write_scope(mut self, write_scope: WriteScope) -> Self {
        self.write_scope = Some(write_scope);
        self
    }

    pub const fn with_execution_isolation(
        mut self,
        execution_isolation: ExecutionIsolation,
    ) -> Self {
        self.execution_isolation = Some(execution_isolation);
        self
    }

    pub const fn with_decision(mut self, decision: Decision) -> Self {
        self.decision = Some(decision);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PolicyProfileV1 {
    pub version: SpecVersion,
    pub risk_profile: RiskProfile,
    pub write_scope: WriteScope,
    pub execution_isolation: ExecutionIsolation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decision: Option<Decision>,
}

impl PolicyProfileV1 {
    pub const fn new(
        risk_profile: RiskProfile,
        write_scope: WriteScope,
        execution_isolation: ExecutionIsolation,
    ) -> Self {
        Self {
            version: SpecVersion::V1,
            risk_profile,
            write_scope,
            execution_isolation,
            decision: None,
        }
    }

    pub const fn with_decision(mut self, decision: Decision) -> Self {
        self.decision = Some(decision);
        self
    }
}

impl From<PolicyProfileV1> for PolicyMetaV1 {
    fn from(profile: PolicyProfileV1) -> Self {
        Self::from(&profile)
    }
}

impl From<&PolicyProfileV1> for PolicyMetaV1 {
    fn from(profile: &PolicyProfileV1) -> Self {
        let mut meta = PolicyMetaV1::new()
            .with_version()
            .with_risk_profile(profile.risk_profile)
            .with_write_scope(profile.write_scope)
            .with_execution_isolation(profile.execution_isolation);
        if let Some(decision) = profile.decision {
            meta = meta.with_decision(decision);
        }
        meta
    }
}

pub fn policy_meta_schema_document() -> serde_json::Value {
    export_schema_document::<PolicyMetaV1>(POLICY_META_SCHEMA_ID, POLICY_META_SCHEMA_DESCRIPTION)
}

pub fn policy_profile_schema_document() -> serde_json::Value {
    let mut value = policy_meta_schema_document();
    let object = value
        .as_object_mut()
        .expect("generated root schema should be an object");

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

    value
}

pub fn schema_documents() -> [(&'static str, serde_json::Value); 2] {
    [
        (POLICY_META_SCHEMA_FILE, policy_meta_schema_document()),
        (POLICY_PROFILE_SCHEMA_FILE, policy_profile_schema_document()),
    ]
}

pub fn policy_meta_typescript_bindings() -> String {
    let declarations = [
        <SpecVersion as TS>::decl(),
        <RiskProfile as TS>::decl(),
        <WriteScope as TS>::decl(),
        <ExecutionIsolation as TS>::decl(),
        <Decision as TS>::decl(),
        <PolicyMetaV1 as TS>::decl(),
        <PolicyProfileV1 as TS>::decl(),
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
    output
}

pub fn write_schema_dir(output_dir: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(output_dir)?;
    converge_generated_dir(
        output_dir,
        schema_documents()
            .into_iter()
            .map(|(file_name, _)| file_name),
        "schema",
    )?;
    for (file_name, schema) in schema_documents() {
        let path = output_dir.join(file_name);
        fs::write(path, render_schema(&schema)?)?;
    }
    Ok(())
}

pub fn check_schema_dir(output_dir: &Path) -> Result<(), Box<dyn Error>> {
    let mut drift = Vec::<String>::new();
    record_unexpected_entries(
        output_dir,
        schema_documents()
            .into_iter()
            .map(|(file_name, _)| file_name),
        "schema",
        &mut drift,
    )?;

    for (file_name, expected) in schema_documents() {
        let path = output_dir.join(file_name);
        let actual: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&path)
                .map_err(|err| format!("failed to read {}: {err}", path.display()))?,
        )
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;

        if actual != expected {
            drift.push(file_name.to_string());
        }
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "schema files out of sync: {}. Run `cargo run -p policy-meta --bin export-artifacts`.",
            drift.join(", ")
        )
        .into())
    }
}

pub fn write_typescript_bindings(output_dir: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(output_dir)?;
    converge_generated_dir(
        output_dir,
        std::iter::once(POLICY_META_TYPES_FILE),
        "typescript bindings",
    )?;
    fs::write(
        output_dir.join(POLICY_META_TYPES_FILE),
        policy_meta_typescript_bindings(),
    )?;
    Ok(())
}

pub fn check_typescript_bindings(output_dir: &Path) -> Result<(), Box<dyn Error>> {
    let mut drift = Vec::<String>::new();
    record_unexpected_entries(
        output_dir,
        std::iter::once(POLICY_META_TYPES_FILE),
        "typescript bindings",
        &mut drift,
    )?;

    let path = output_dir.join(POLICY_META_TYPES_FILE);
    let actual = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let expected = policy_meta_typescript_bindings();

    if actual != expected {
        drift.push(path.display().to_string());
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "typescript bindings out of sync: {}. Run `cargo run -p policy-meta --bin export-artifacts`.",
            drift.join(", ")
        )
        .into())
    }
}

fn converge_generated_dir<'a>(
    output_dir: &Path,
    expected_files: impl IntoIterator<Item = &'a str>,
    artifact_kind: &str,
) -> Result<(), Box<dyn Error>> {
    let expected = expected_file_names(expected_files);
    for entry in fs::read_dir(output_dir)
        .map_err(|err| format!("failed to read {}: {err}", output_dir.display()))?
    {
        let entry =
            entry.map_err(|err| format!("failed to inspect {}: {err}", output_dir.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if expected.contains(name) {
            continue;
        }

        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("failed to inspect {}: {err}", path.display()))?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| {
                format!(
                    "failed to remove stale {artifact_kind} directory {}: {err}",
                    path.display()
                )
            })?;
        } else {
            fs::remove_file(&path).map_err(|err| {
                format!(
                    "failed to remove stale {artifact_kind} file {}: {err}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn record_unexpected_entries<'a>(
    output_dir: &Path,
    expected_files: impl IntoIterator<Item = &'a str>,
    artifact_kind: &str,
    drift: &mut Vec<String>,
) -> Result<(), Box<dyn Error>> {
    let expected = expected_file_names(expected_files);
    for entry in fs::read_dir(output_dir)
        .map_err(|err| format!("failed to read {}: {err}", output_dir.display()))?
    {
        let entry =
            entry.map_err(|err| format!("failed to inspect {}: {err}", output_dir.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if expected.contains(name) {
            continue;
        }
        drift.push(format!(
            "unexpected stale {artifact_kind} entry {}",
            entry.path().display()
        ));
    }
    Ok(())
}

fn expected_file_names<'a>(files: impl IntoIterator<Item = &'a str>) -> BTreeSet<&'a str> {
    files.into_iter().collect()
}

fn render_schema(schema: &serde_json::Value) -> Result<String, serde_json::Error> {
    let mut rendered = serde_json::to_string_pretty(schema)?;
    rendered.push('\n');
    Ok(rendered)
}

fn export_schema_document<T: JsonSchema>(
    schema_id: &'static str,
    description: &'static str,
) -> serde_json::Value {
    let settings = SchemaSettings::draft2019_09().with(|settings| {
        settings.option_nullable = false;
        settings.option_add_null_type = false;
        settings.inline_subschemas = true;
    });

    let mut value = serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
        .expect("serialize generated schema");
    let object = value
        .as_object_mut()
        .expect("generated root schema should be an object");

    object.insert(
        "$schema".to_string(),
        serde_json::Value::String(POLICY_META_SCHEMA_URI.to_string()),
    );
    object.insert(
        "$id".to_string(),
        serde_json::Value::String(schema_id.to_string()),
    );
    object.insert(
        "title".to_string(),
        serde_json::Value::String(T::schema_name()),
    );
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

    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use serde_json::{Value, json};

    #[test]
    fn risk_profile_rejects_noncanonical_values() {
        let err = serde_json::from_str::<RiskProfile>("\"yolo\"").expect_err("reject yolo alias");
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn write_scope_rejects_noncanonical_values() {
        for alias in ["workspace", "global", "unrestricted"] {
            let err = serde_json::from_str::<WriteScope>(&format!("\"{alias}\"")).expect_err(alias);
            assert!(
                err.to_string().contains("unknown variant"),
                "alias={alias} err={err}"
            );
        }
    }

    #[test]
    fn canonical_serialization_uses_normalized_values() {
        let danger = serde_json::to_string(&RiskProfile::Danger).expect("serialize danger");
        let full = serde_json::to_string(&WriteScope::FullAccess).expect("serialize full");

        assert_eq!(danger, "\"danger\"");
        assert_eq!(full, "\"full_access\"");
    }

    #[test]
    fn spec_version_serializes_as_integer_one() {
        let value = serde_json::to_string(&SpecVersion::V1).expect("serialize version");
        assert_eq!(value, "1");
    }

    #[test]
    fn spec_version_rejects_unknown_values() {
        let err = serde_json::from_str::<SpecVersion>("2").expect_err("reject non-v1 version");
        assert!(err.to_string().contains("integer 1"));
    }

    #[test]
    fn policy_meta_rejects_noncanonical_values() {
        let err = serde_json::from_value::<PolicyMetaV1>(json!({
            "version": 1,
            "risk_profile": "yolo",
            "write_scope": "global",
            "execution_isolation": "strict",
            "decision": "prompt_strict"
        }))
        .expect_err("reject noncanonical values");

        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn policy_profile_constructor_sets_version() {
        let profile = PolicyProfileV1::new(
            RiskProfile::Standard,
            WriteScope::WorkspaceWrite,
            ExecutionIsolation::BestEffort,
        )
        .with_decision(Decision::Prompt);

        assert_eq!(profile.version, SpecVersion::V1);
        assert_eq!(profile.decision, Some(Decision::Prompt));
    }

    #[test]
    fn policy_meta_builders_emit_expected_fragment() {
        let meta = PolicyMetaV1::new()
            .with_version()
            .with_risk_profile(RiskProfile::Standard)
            .with_write_scope(WriteScope::WorkspaceWrite)
            .with_execution_isolation(ExecutionIsolation::BestEffort)
            .with_decision(Decision::Prompt);

        assert_eq!(
            serde_json::to_value(meta).expect("serialize policy meta"),
            json!({
                "version": 1,
                "risk_profile": "standard",
                "write_scope": "workspace_write",
                "execution_isolation": "best_effort",
                "decision": "prompt"
            })
        );
    }

    #[test]
    fn policy_profile_projects_to_policy_meta() {
        let profile = PolicyProfileV1::new(
            RiskProfile::Standard,
            WriteScope::WorkspaceWrite,
            ExecutionIsolation::BestEffort,
        )
        .with_decision(Decision::Prompt);

        assert_eq!(
            PolicyMetaV1::from(&profile),
            PolicyMetaV1::new()
                .with_version()
                .with_risk_profile(RiskProfile::Standard)
                .with_write_scope(WriteScope::WorkspaceWrite)
                .with_execution_isolation(ExecutionIsolation::BestEffort)
                .with_decision(Decision::Prompt)
        );
        assert_eq!(
            PolicyMetaV1::from(profile.clone()),
            PolicyMetaV1::from(&profile)
        );
    }

    #[test]
    fn generated_policy_meta_schema_matches_contract() {
        assert_policy_meta_schema(&policy_meta_schema_document());
    }

    #[test]
    fn generated_policy_profile_schema_matches_contract() {
        assert_policy_profile_schema(&policy_profile_schema_document());
    }

    #[test]
    fn checked_in_policy_meta_schema_matches_contract() {
        let checked_in = checked_in_schema(POLICY_META_SCHEMA_FILE);
        assert_policy_meta_schema(&checked_in);
        assert_eq!(checked_in, policy_meta_schema_document());
    }

    #[test]
    fn checked_in_policy_profile_schema_matches_contract() {
        let checked_in = checked_in_schema(POLICY_PROFILE_SCHEMA_FILE);
        assert_policy_profile_schema(&checked_in);
        assert_eq!(checked_in, policy_profile_schema_document());
    }

    #[test]
    fn generated_typescript_bindings_contain_core_types() {
        let bindings = policy_meta_typescript_bindings();
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
        assert_eq!(checked_in, policy_meta_typescript_bindings());
    }

    #[test]
    fn checked_in_profiles_match_expected_presets() {
        let cases = [
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
        ];

        for (file_name, expected) in cases {
            assert_eq!(checked_in_profile(file_name), expected, "{file_name}");
        }
    }

    #[test]
    fn write_schema_dir_removes_stale_entries() {
        let dir = tempdir().expect("tempdir");
        let stale_file = dir.path().join("stale-schema.json");
        let stale_dir = dir.path().join("legacy");
        std::fs::write(&stale_file, "{}\n").expect("write stale file");
        std::fs::create_dir(&stale_dir).expect("create stale dir");

        write_schema_dir(dir.path()).expect("write schema dir");

        assert!(!stale_file.exists(), "stale schema file should be removed");
        assert!(!stale_dir.exists(), "stale schema dir should be removed");
        for (file_name, expected) in schema_documents() {
            let path = dir.path().join(file_name);
            let actual: Value =
                serde_json::from_str(&std::fs::read_to_string(path).expect("read schema file"))
                    .expect("parse schema file");
            assert_eq!(actual, expected, "{file_name}");
        }
    }

    #[test]
    fn check_schema_dir_rejects_stale_entries() {
        let dir = tempdir().expect("tempdir");
        write_schema_dir(dir.path()).expect("write schema dir");
        let stale_file = dir.path().join("stale-schema.json");
        std::fs::write(&stale_file, "{}\n").expect("write stale file");

        let err = check_schema_dir(dir.path()).expect_err("stale schema entry should fail");
        let msg = err.to_string();
        assert!(msg.contains("unexpected stale schema entry"), "{msg}");
        assert!(msg.contains("stale-schema.json"), "{msg}");
    }

    #[test]
    fn write_typescript_bindings_removes_stale_entries() {
        let dir = tempdir().expect("tempdir");
        let stale_file = dir.path().join("policy-meta.old.d.ts");
        let stale_dir = dir.path().join("legacy");
        std::fs::write(&stale_file, "export type Old = never;\n").expect("write stale file");
        std::fs::create_dir(&stale_dir).expect("create stale dir");

        write_typescript_bindings(dir.path()).expect("write types dir");

        assert!(!stale_file.exists(), "stale binding file should be removed");
        assert!(!stale_dir.exists(), "stale binding dir should be removed");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(POLICY_META_TYPES_FILE))
                .expect("read bindings"),
            policy_meta_typescript_bindings()
        );
    }

    #[test]
    fn check_typescript_bindings_rejects_stale_entries() {
        let dir = tempdir().expect("tempdir");
        write_typescript_bindings(dir.path()).expect("write types dir");
        let stale_file = dir.path().join("policy-meta.old.d.ts");
        std::fs::write(&stale_file, "export type Old = never;\n").expect("write stale file");

        let err =
            check_typescript_bindings(dir.path()).expect_err("stale bindings entry should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("unexpected stale typescript bindings entry"),
            "{msg}"
        );
        assert!(msg.contains("policy-meta.old.d.ts"), "{msg}");
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
        assert_eq!(schema["title"], json!("PolicyMetaV1"));
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_required_fields(schema, &[]);
        assert_common_properties(schema);
    }

    fn assert_policy_profile_schema(schema: &Value) {
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

    #[test]
    fn policy_profile_schema_reuses_fragment_property_domain() {
        let fragment = policy_meta_schema_document();
        let profile = policy_profile_schema_document();

        assert_eq!(profile["properties"], fragment["properties"]);
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
