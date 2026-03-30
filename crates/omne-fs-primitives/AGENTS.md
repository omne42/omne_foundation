# omne-fs-primitives AGENTS Map

This file is only a map. The local `docs/` tree is the system of record.

## Read First

- Overview: `README.md`
- Docs entrypoint: `docs/docs-system-map.md`
- Boundaries: `docs/architecture/system-boundaries.md`
- Source layout: `docs/architecture/source-layout.md`
- Workspace boundaries: `../../docs/workspace-crate-boundaries.md`

## Edit Rules

- Keep `AGENTS.md` short.
- Primitive boundary changes update `system-boundaries.md`.
- Source file responsibility changes update `source-layout.md`.

## Verify

- `cargo test -p omne-fs-primitives`
- `../../scripts/check-docs-system.sh`
