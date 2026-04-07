use std::borrow::Cow;

use schemars::{
    JsonSchema,
    r#gen::SchemaGenerator,
    schema::{InstanceType, Schema, SchemaObject, SingleOrVec},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as DeError, Unexpected},
};
use ts_rs::TS;

pub const POLICY_META_SPEC_VERSION: u8 = 1;

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

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

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
}
