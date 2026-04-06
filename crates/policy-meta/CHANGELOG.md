# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Fixed

- `policy-meta` 的纯 artifact 生成边界现在返回可处理错误而不是依赖 `expect(...)` panic；当 schema/TypeScript 生成器输出不再满足库内不变量时，调用方和 `export-*` CLI 会拿到 typed error。
- Stop exporting artifact export/check filesystem workflows from the `policy-meta` library; that boundary now lives under `src/bin/shared/` so the contract crate only exposes reusable policy semantics and pure generated outputs.
- Make `export-artifacts --check` fail closed for stale files under `schema/`, `bindings/`, and `profiles/`, and let regeneration prune stale artifacts back to the canonical checked-in set.
- Align the checked-in JSON Schema dialect with the actual `schemars` 2019-09 generator output instead of advertising 2020-12.
- Keep typed artifact/CLI errors on the binary side instead of exposing them from the public contract crate API, so argument mistakes and artifact failures stop flattening into erased boundaries without leaking filesystem workflow into library consumers.

### Changed

- `README.md` 现在把 `CHANGELOG.md` 列为正式入口，避免调用方或维护者只看到 schema / bindings / profiles，却漏掉 crate 自己的变更记录位置。
