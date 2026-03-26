// This file was generated from crates/policy-meta by ts-rs-backed export code. Do not edit manually.

export type SpecVersion = 1;

export type RiskProfile = "safe" | "standard" | "proactive" | "danger";

export type WriteScope = "read_only" | "workspace_write" | "full_access";

export type ExecutionIsolation = "none" | "best_effort" | "strict";

export type Decision = "allow" | "prompt" | "prompt_strict" | "deny";

export type PolicyMetaV1 = { version?: SpecVersion, risk_profile?: RiskProfile, write_scope?: WriteScope, execution_isolation?: ExecutionIsolation, decision?: Decision, };

export type PolicyProfileV1 = { version: SpecVersion, risk_profile: RiskProfile, write_scope: WriteScope, execution_isolation: ExecutionIsolation, decision?: Decision, };
