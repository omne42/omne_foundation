use std::{
    borrow::Cow,
    collections::BTreeSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

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
pub const POLICY_META_SCHEMA_URI: &str = "https://json-schema.org/draft/2019-09/schema";
pub const POLICY_META_SCHEMA_DESCRIPTION: &str = "Canonical policy metadata fragment. Canonical fields are reusable across repositories; field presence requirements are defined by embedding contracts. Optional version metadata may be included by persisted artifacts and preset files.";
pub const POLICY_PROFILE_SCHEMA_DESCRIPTION: &str =
    "Versioned preset profile built on top of the canonical policy metadata fragment.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactKind {
    Schema,
    TypescriptBinding,
    Profile,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::TypescriptBinding => "typescript binding",
            Self::Profile => "profile",
        }
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub enum ArtifactError {
    CreateDir {
        path: PathBuf,
        source: io::Error,
    },
    ReadDir {
        path: PathBuf,
        source: io::Error,
    },
    InspectDir {
        path: PathBuf,
        source: io::Error,
    },
    ReadFile {
        path: PathBuf,
        source: io::Error,
    },
    WriteFile {
        path: PathBuf,
        source: io::Error,
    },
    RemoveArtifact {
        path: PathBuf,
        source: io::Error,
    },
    ParseJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    RenderSchema {
        source: serde_json::Error,
    },
    RenderProfile {
        source: serde_yaml::Error,
    },
    UnexpectedArtifacts {
        kind: ArtifactKind,
        files: Vec<String>,
    },
    Drift {
        kind: ArtifactKind,
        files: Vec<String>,
    },
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, source } => {
                write!(
                    f,
                    "failed to create artifact dir {}: {source}",
                    path.display()
                )
            }
            Self::ReadDir { path, source } => {
                write!(
                    f,
                    "failed to read artifact dir {}: {source}",
                    path.display()
                )
            }
            Self::InspectDir { path, source } => {
                write!(
                    f,
                    "failed to inspect artifact dir {}: {source}",
                    path.display()
                )
            }
            Self::ReadFile { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::WriteFile { path, source } => {
                write!(f, "failed to write {}: {source}", path.display())
            }
            Self::RemoveArtifact { path, source } => {
                write!(
                    f,
                    "failed to remove stale generated artifact {}: {source}",
                    path.display()
                )
            }
            Self::ParseJson { path, source } => {
                write!(f, "failed to parse {}: {source}", path.display())
            }
            Self::RenderSchema { source } => write!(f, "failed to render schema: {source}"),
            Self::RenderProfile { source } => write!(f, "failed to render profile: {source}"),
            Self::UnexpectedArtifacts { kind, files } => write!(
                f,
                "unexpected {kind} artifacts present: {}. Run `cargo run -p policy-meta --bin export-artifacts` to regenerate the canonical artifact set.",
                files.join(", ")
            ),
            Self::Drift { kind, files } => write!(
                f,
                "{} artifacts out of sync: {}. Run `cargo run -p policy-meta --bin export-artifacts`.",
                kind,
                files.join(", ")
            ),
        }
    }
}

impl std::error::Error for ArtifactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateDir { source, .. }
            | Self::ReadDir { source, .. }
            | Self::InspectDir { source, .. }
            | Self::ReadFile { source, .. }
            | Self::WriteFile { source, .. }
            | Self::RemoveArtifact { source, .. } => Some(source),
            Self::ParseJson { source, .. } => Some(source),
            Self::RenderSchema { source } => Some(source),
            Self::RenderProfile { source } => Some(source),
            Self::UnexpectedArtifacts { .. } | Self::Drift { .. } => None,
        }
    }
}

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

pub fn write_schema_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(output_dir).map_err(|source| ArtifactError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;
    prune_unexpected_artifacts(
        output_dir,
        schema_documents().iter().map(|(file_name, _)| *file_name),
    )?;
    for (file_name, schema) in schema_documents() {
        let path = output_dir.join(file_name);
        fs::write(&path, render_schema(&schema)?)
            .map_err(|source| ArtifactError::WriteFile { path, source })?;
    }
    Ok(())
}

pub fn check_schema_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    check_no_unexpected_artifacts(
        output_dir,
        schema_documents().iter().map(|(file_name, _)| *file_name),
        ArtifactKind::Schema,
    )?;
    let mut drift = Vec::<String>::new();

    for (file_name, expected) in schema_documents() {
        let path = output_dir.join(file_name);
        let contents = fs::read_to_string(&path).map_err(|source| ArtifactError::ReadFile {
            path: path.clone(),
            source,
        })?;
        let actual: serde_json::Value =
            serde_json::from_str(&contents).map_err(|source| ArtifactError::ParseJson {
                path: path.clone(),
                source,
            })?;

        if actual != expected {
            drift.push(file_name.to_string());
        }
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(ArtifactError::Drift {
            kind: ArtifactKind::Schema,
            files: drift,
        })
    }
}

pub fn write_typescript_bindings(output_dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(output_dir).map_err(|source| ArtifactError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;
    prune_unexpected_artifacts(output_dir, [POLICY_META_TYPES_FILE])?;
    let path = output_dir.join(POLICY_META_TYPES_FILE);
    fs::write(&path, policy_meta_typescript_bindings())
        .map_err(|source| ArtifactError::WriteFile { path, source })?;
    Ok(())
}

pub fn check_typescript_bindings(output_dir: &Path) -> Result<(), ArtifactError> {
    check_no_unexpected_artifacts(
        output_dir,
        [POLICY_META_TYPES_FILE],
        ArtifactKind::TypescriptBinding,
    )?;
    let path = output_dir.join(POLICY_META_TYPES_FILE);
    let actual = fs::read_to_string(&path).map_err(|source| ArtifactError::ReadFile {
        path: path.clone(),
        source,
    })?;
    let expected = policy_meta_typescript_bindings();

    if actual == expected {
        Ok(())
    } else {
        Err(ArtifactError::Drift {
            kind: ArtifactKind::TypescriptBinding,
            files: vec![POLICY_META_TYPES_FILE.to_string()],
        })
    }
}

pub fn write_profiles_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(output_dir).map_err(|source| ArtifactError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;
    prune_unexpected_artifacts(
        output_dir,
        profile_documents().iter().map(|(file_name, _)| *file_name),
    )?;
    for (file_name, profile) in profile_documents() {
        let path = output_dir.join(file_name);
        fs::write(&path, render_profile(&profile)?)
            .map_err(|source| ArtifactError::WriteFile { path, source })?;
    }
    Ok(())
}

pub fn check_profiles_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    check_no_unexpected_artifacts(
        output_dir,
        profile_documents().iter().map(|(file_name, _)| *file_name),
        ArtifactKind::Profile,
    )?;
    let mut drift = Vec::<String>::new();

    for (file_name, expected) in profile_documents() {
        let path = output_dir.join(file_name);
        let actual = fs::read_to_string(&path).map_err(|source| ArtifactError::ReadFile {
            path: path.clone(),
            source,
        })?;
        if actual != render_profile(&expected)? {
            drift.push(file_name.to_string());
        }
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(ArtifactError::Drift {
            kind: ArtifactKind::Profile,
            files: drift,
        })
    }
}

fn render_schema(schema: &serde_json::Value) -> Result<String, ArtifactError> {
    let mut rendered = serde_json::to_string_pretty(schema)
        .map_err(|source| ArtifactError::RenderSchema { source })?;
    rendered.push('\n');
    Ok(rendered)
}

fn render_profile(profile: &PolicyProfileV1) -> Result<String, ArtifactError> {
    let rendered =
        serde_yaml::to_string(profile).map_err(|source| ArtifactError::RenderProfile { source })?;
    Ok(rendered
        .strip_prefix("---\n")
        .unwrap_or(rendered.as_str())
        .to_string())
}

fn prune_unexpected_artifacts<'a>(
    output_dir: &Path,
    expected_files: impl IntoIterator<Item = &'a str>,
) -> Result<(), ArtifactError> {
    let expected = expected_artifact_names(expected_files);
    for entry in fs::read_dir(output_dir).map_err(|source| ArtifactError::ReadDir {
        path: output_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ArtifactError::InspectDir {
            path: output_dir.to_path_buf(),
            source,
        })?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if expected.contains(file_name.as_ref()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| ArtifactError::InspectDir {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path)
                .map_err(|source| ArtifactError::RemoveArtifact { path, source })?;
        } else {
            fs::remove_file(&path)
                .map_err(|source| ArtifactError::RemoveArtifact { path, source })?;
        }
    }
    Ok(())
}

fn check_no_unexpected_artifacts<'a>(
    output_dir: &Path,
    expected_files: impl IntoIterator<Item = &'a str>,
    artifact_kind: ArtifactKind,
) -> Result<(), ArtifactError> {
    let expected = expected_artifact_names(expected_files);
    let mut unexpected = Vec::new();
    for entry in fs::read_dir(output_dir).map_err(|source| ArtifactError::ReadDir {
        path: output_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ArtifactError::InspectDir {
            path: output_dir.to_path_buf(),
            source,
        })?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !expected.contains(file_name.as_ref()) {
            unexpected.push(file_name.into_owned());
        }
    }
    unexpected.sort();
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(ArtifactError::UnexpectedArtifacts {
        kind: artifact_kind,
        files: unexpected,
    })
}

fn expected_artifact_names<'a>(
    expected_files: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<&'a str> {
    expected_files.into_iter().collect()
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
    fn check_schema_dir_rejects_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-schema-check");
        write_schema_dir(dir.path()).expect("write canonical schema dir");
        let stale = dir.path().join("stale-policy-meta.json");
        std::fs::write(&stale, "{}\n").expect("write stale schema artifact");

        let err =
            check_schema_dir(dir.path()).expect_err("stale schema artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::UnexpectedArtifacts { kind: ArtifactKind::Schema, ref files }
            if files == &vec!["stale-policy-meta.json".to_string()]
        ));
    }

    #[test]
    fn write_schema_dir_prunes_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-schema-write");
        let stale = dir.path().join("obsolete.json");
        std::fs::write(&stale, "{}\n").expect("write stale schema artifact");

        write_schema_dir(dir.path()).expect("write canonical schema dir");

        assert!(
            !stale.exists(),
            "regeneration should remove stale schema artifacts instead of leaving drift behind"
        );
    }

    #[test]
    fn check_typescript_bindings_rejects_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-bindings-check");
        write_typescript_bindings(dir.path()).expect("write canonical bindings dir");
        let stale = dir.path().join("obsolete.d.ts");
        std::fs::write(&stale, "// stale\n").expect("write stale bindings artifact");

        let err = check_typescript_bindings(dir.path())
            .expect_err("stale bindings artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::UnexpectedArtifacts { kind: ArtifactKind::TypescriptBinding, ref files }
            if files == &vec!["obsolete.d.ts".to_string()]
        ));
    }

    #[test]
    fn check_profiles_dir_rejects_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-profiles-check");
        write_profiles_dir(dir.path()).expect("write canonical profiles dir");
        let stale = dir.path().join("obsolete.yaml");
        std::fs::write(&stale, "version: 1\n").expect("write stale profile artifact");

        let err =
            check_profiles_dir(dir.path()).expect_err("stale profile artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::UnexpectedArtifacts { kind: ArtifactKind::Profile, ref files }
            if files == &vec!["obsolete.yaml".to_string()]
        ));
    }

    #[test]
    fn check_schema_dir_reports_parse_failures_structurally() {
        let dir = unique_tempdir("policy-meta-schema-parse");
        write_schema_dir(dir.path()).expect("write canonical schema dir");
        let path = dir.path().join(POLICY_META_SCHEMA_FILE);
        std::fs::write(&path, "{not-json\n").expect("write invalid schema artifact");

        let err = check_schema_dir(dir.path()).expect_err("invalid json should fail check");
        assert!(matches!(
            err,
            ArtifactError::ParseJson { path: ref actual_path, .. } if actual_path == &path
        ));
    }

    #[test]
    fn check_typescript_bindings_reports_drift_structurally() {
        let dir = unique_tempdir("policy-meta-bindings-drift");
        write_typescript_bindings(dir.path()).expect("write canonical bindings dir");
        let path = dir.path().join(POLICY_META_TYPES_FILE);
        std::fs::write(&path, "// drifted\n").expect("write drifted bindings artifact");

        let err = check_typescript_bindings(dir.path())
            .expect_err("drifted bindings artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::Drift { kind: ArtifactKind::TypescriptBinding, ref files }
            if files == &vec![POLICY_META_TYPES_FILE.to_string()]
        ));
    }

    #[test]
    fn checked_in_profiles_match_expected_presets() {
        for (file_name, expected) in profile_documents() {
            assert_eq!(checked_in_profile(file_name), expected, "{file_name}");
        }
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

    fn unique_tempdir(prefix: &str) -> tempfile_dir::TempDir {
        tempfile_dir::TempDir::new(prefix).expect("create tempdir")
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

    mod tempfile_dir {
        use std::{
            env, fs,
            path::{Path, PathBuf},
            sync::atomic::{AtomicU64, Ordering},
        };

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn new(prefix: &str) -> std::io::Result<Self> {
                let path = env::temp_dir().join(format!(
                    "{prefix}-{}-{}",
                    std::process::id(),
                    NEXT_ID.fetch_add(1, Ordering::Relaxed)
                ));
                if path.exists() {
                    fs::remove_dir_all(&path)?;
                }
                fs::create_dir_all(&path)?;
                Ok(Self { path })
            }

            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }
}
