# Policy Meta Spec v1

## Goal

Unify policy naming and semantics across repositories while keeping implementation details decoupled.

This specification is the canonical semantic contract for policy metadata shared across policy-producing and policy-consuming repositories.

The canonical contract is intentionally split into:

- a reusable metadata fragment (`policy-meta.v1.json`),
- versioned preset/profile artifacts built on top of that fragment.

## Canonical Fields

- `risk_profile`: `safe | standard | proactive | danger`
- `write_scope`: `read_only | workspace_write | full_access`
- `execution_isolation`: `none | best_effort | strict`
- `decision`: `allow | prompt | prompt_strict | deny`

Optional artifact metadata:

- `version`: `1`

Canonical values are case-sensitive snake_case tokens.

## Contract Shape

- `schema/policy-meta.v1.json` defines the canonical metadata fragment.
- The fragment keeps canonical policy fields reusable and does not force every field to be present.
- `version` is optional in the fragment so the same metadata shape can be embedded into larger contracts, but allowed for persisted artifacts.
- `schema/policy-profile.v1.json` defines a versioned preset/profile object and requires:
  - `version`
  - `risk_profile`
  - `write_scope`
  - `execution_isolation`
- `bindings/policy-meta.d.ts` is a synchronized TypeScript artifact exported from the Rust types crate.

## Naming Rationale

- `workspace_write` keeps workspace-scoped write intent explicit.
- `full_access` is clear and auditable for high-risk operations.
- `execution_isolation` describes guarantee strength, not implementation mechanism.

## Parsing Rules

- Parsers MUST accept only canonical values.
- Persisted output MUST contain only canonical values.
- Unknown values MUST fail closed.

## Stability Rules

- Existing canonical values are immutable in meaning.
- Backward-incompatible rename/removal requires major version bump.
- Additive evolution is preferred (new fields/new enum values).

## Out of Scope

- Runtime policy decision engine behavior.
- Transport API protocol definitions.
- Platform-specific sandbox implementation details.

## Normative Artifacts

- Canonical fragment schema: `schema/policy-meta.v1.json`
- Profile schema: `schema/policy-profile.v1.json`
- TypeScript bindings: `bindings/policy-meta.d.ts`
- Profiles: `profiles/*.yaml`
- Rust types: `src/lib.rs`
