# policy-meta

源码入口：[`src/lib.rs`](./src/lib.rs)  
补充规范：[`SPEC.md`](./SPEC.md)

## 领域

`policy-meta` 是跨仓库共享的策略元数据契约层。

它不实现执行策略本身，而是固定“策略字段叫什么、值域是什么、schema 和 TypeScript 绑定长什么样”。

## 边界

负责：

- `PolicyMetaV1` 与 `PolicyProfileV1`
- `risk_profile`、`write_scope`、`execution_isolation`、`decision` 的 canonical 语义
- checked-in JSON Schema
- checked-in TypeScript bindings
- baseline profiles
- artifact export / drift check 二进制

不负责：

- runtime 决策引擎
- mode / role / approval 的上层组合
- sandbox 或 filesystem enforcement
- transport protocol envelope
- 产品级配置 merge 逻辑

## 范围

覆盖：

- `src/lib.rs` 中的 Rust 类型与 schema 导出逻辑
- `src/bin/export-artifacts.rs`
- `src/bin/export-schemas.rs`
- `src/bin/export-types.rs`
- `schema/`
- `bindings/`
- `profiles/`
- `SPEC.md`

不覆盖：

- 调用方如何把这些字段嵌进自己的配置格式
- 对命令、文件系统或审批流程的真实执行

## 结构设计

- `src/lib.rs`
  - canonical enum / struct 定义
  - JSON Schema 生成
  - TypeScript declarations 导出
  - checked-in artifact drift 检查
- `src/bin/export-artifacts.rs`
  - 同时导出并校验 `schema/` 与 `bindings/`
- `src/bin/export-schemas.rs`
  - 只处理 `schema/`
- `src/bin/export-types.rs`
  - 只处理 `bindings/`
- `schema/`
  - checked-in schema contracts
- `bindings/`
  - checked-in TypeScript bindings
- `profiles/`
  - 基础预设 profile

## 与其他 crate 的关系

- 当前不依赖 `omne_foundation` 内其他 crate
- 主要被外部 workspace 作为共享 contract crate 使用
- 它的目标是让“策略语义”与“策略执行实现”解耦

## Core Artifacts

- Spec: `SPEC.md`
- Rust types: `src/lib.rs`
- Export binaries:
  - `src/bin/export-artifacts.rs`
  - `src/bin/export-schemas.rs`
  - `src/bin/export-types.rs`
- Canonical fragment schema: `schema/policy-meta.v1.json`
- Profile schema: `schema/policy-profile.v1.json`
- TypeScript bindings: `bindings/policy-meta.d.ts`
- Baseline profiles: `profiles/*.yaml`

checked-in schema 和 TypeScript bindings 由 Rust 类型定义导出，并通过 `export-artifacts` 做同步校验。

## Local Validation

下面命令默认从 `omne_foundation` 仓库根目录执行：

```bash
cargo run -p policy-meta --bin export-artifacts
cargo run -p policy-meta --bin export-artifacts -- --check
cargo test -p policy-meta
./scripts/check-workspace.sh asset-checks policy-meta
```
