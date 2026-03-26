# Hook 与质量门禁

这个文件说明 `omne_foundation` 当前在提交阶段执行的主要 gate。

它覆盖的重点不是 crate 业务语义，而是仓库在提交时自动检查什么、为什么检查、哪些情况需要人工判断。

## 执行入口

当前主要入口是：

- [`../../githooks/pre-commit`](../../githooks/pre-commit)
- [`../../githooks/commit-msg`](../../githooks/commit-msg)
- [`../../scripts/pre_commit_check/`](../../scripts/pre_commit_check/)
- [`../../scripts/version_policy/`](../../scripts/version_policy/)
- [`../../scripts/workspace_check/`](../../scripts/workspace_check/)
- [`../../scripts/check-version-policy.py`](../../scripts/check-version-policy.py)
- [`../../scripts/check-workspace.sh`](../../scripts/check-workspace.sh)

其中：

- `githooks/pre-commit` 负责把控制权交给 `scripts/pre_commit_check/__main__.py`
- `githooks/commit-msg` 负责分支名、Conventional Commits 和 major bump 的 breaking marker
- `scripts/pre_commit_check/` 是当前 `pre-commit` 的 canonical 实现
- `scripts/version_policy/` 是版本策略的共享核心
- `scripts/workspace_check/` 是 workspace 质量检查的共享核心
- `scripts/check-version-policy.py` 与 `scripts/check-workspace.sh` 是对这两个共享核心的薄入口

## `pre-commit` 当前做什么

按执行顺序，`pre-commit` 主要做这些事：

1. 检查当前分支名是否合法。
2. 如果没有 staged 文件，直接退出。
3. 运行版本策略检查。
4. 检查是否同时更新了 changelog。
5. 拒绝 changelog-only 提交。
6. 默认拒绝修改已发布 changelog 段落。
7. 运行 workspace 级文档系统与 Rust checks。
8. 对特定 crate 触发额外 asset checks。

## 版本策略 gate

`pre-commit` 会通过 `scripts/version_policy/` 运行版本策略检查；顶层 `check-version-policy.py` 只是对应的薄入口。

它当前做的机械检查包括：

- `crates/` 下不允许混用两种版本声明模式：
  - 所有 crate 都用 `version.workspace = true`
  - 或所有 crate 都显式写 `package.version`
- 默认拒绝非 `0` 大版本的 major bump，除非显式设置：
  - `OMNE_ALLOW_MAJOR_VERSION_BUMP=1`
- 对 staged crate 做 Rust public API diff
- 当 crate 处于非 `0` 大版本时：
  - breaking public API 变更必须提升大版本
  - additive public API 变更至少要提升小版本

它的自动化覆盖范围目前是 Rust public API，不自动涵盖：

- CLI 语义
- 配置格式
- 协议行为
- 运行时语义

这些部分仍然需要人工工程判断，详见 [`./版本与兼容.md`](./版本与兼容.md)。

## changelog gate

`pre-commit` 还会机械执行 changelog 规则：

- 共享检查引擎本身支持 `root-package repository` 和 `crate-package directory` 两种形态
- 当前仓库实际会自动识别为 crate-package directory
- 每个发生实际变更的 crate 都必须更新自己的 `crates/<name>/CHANGELOG.md`
- 根级 `CHANGELOG.md` 不属于当前仓库的 changelog 入口
- 默认只允许修改 `[Unreleased]`
- release 场景下可显式使用：
  - `OMNE_ALLOW_CHANGELOG_RELEASE_EDIT=1`

详见 [`./变更记录.md`](./变更记录.md)。

## workspace Rust gate

`pre-commit` 会直接执行 workspace 级 Rust 本地门禁。当前包含：

- 文档系统入口检查
- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --all-features`
- `cargo test --workspace --all-features`

也就是说，提交前默认要满足文档结构、格式、编译和测试这四类本地门禁。

`scripts/workspace_check/` 是这里的共享实现；`scripts/check-workspace.sh` 只是保留给手动执行和 CI 复用的薄入口。

## asset checks

如果 staged 文件命中某些路径，`pre-commit` 还会触发额外资产检查。

### `policy-meta`

当提交涉及这些内容时会触发：

- `crates/policy-meta/Cargo.toml`
- `crates/policy-meta/README.md`
- `crates/policy-meta/SPEC.md`
- `crates/policy-meta/src/*`
- `crates/policy-meta/schema/*`
- `crates/policy-meta/bindings/*`
- `crates/policy-meta/profiles/*`

对应检查包括：

- `cargo run -p policy-meta --bin export-artifacts -- --check`

### `mcp-kit`

当提交涉及这些内容时会触发：

- `crates/mcp-kit/README.md`
- `crates/mcp-kit/CONTRIBUTING.md`
- `crates/mcp-kit/llms.txt`
- `crates/mcp-kit/examples/README.md`
- `crates/mcp-kit/docs/*`
- `crates/mcp-kit/scripts/*`

对应检查包括：

- 校验 `crates/mcp-kit/docs/llms.txt` 与 `crates/mcp-kit/llms.txt` 是否仍与 `docs/SUMMARY.md` 驱动的聚合结果一致
- `mdbook build crates/mcp-kit/docs`

### `notify-kit`

当提交涉及这些内容时会触发：

- `crates/notify-kit/README.md`
- `crates/notify-kit/llms.txt`
- `crates/notify-kit/docs/*`
- `crates/notify-kit/bots/*`
- `crates/notify-kit/scripts/*`

对应检查包括：

- 直接执行 `build-llms-txt.py --check`
- 先 `cargo build --manifest-path crates/notify-kit/Cargo.toml`，再 `mdbook test crates/notify-kit/docs`
- bot entrypoint 的 `node --check`

如果本机没有 `node`，bot syntax check 会被跳过并输出提示。

## `workspace_check` 的其他模式

除了 `local`，共享检查核心还支持：

- `ci`
- `docs-system`
- `asset-checks [all|policy-meta|mcp-kit|notify-kit]`
- `secret-kit-target <target-triple>`

其中：

- `ci` 在 `local` 基础上增加 `clippy` 和全量 asset checks
- `docs-system` 只运行文档系统入口与链接约束检查
- `secret-kit-target` 用于检查 `secret-kit` 的特定 target 编译

## `commit-msg` 当前做什么

`commit-msg` 主要做三件事：

- 再次校验分支名
- 强制 Conventional Commits 格式
- 当检测到 major version bump 时，强制提交消息带 `!`

这使得“版本变化”和“提交意图”保持一致，避免只在 `Cargo.toml` 中隐式引入 breaking change。

## 目的

这些 gate 的作用不是替代设计和评审，而是尽量把高频、明确、适合机械执行的约束前移到提交阶段。

这样做的收益是：

- 让人和 agent 都能尽早获得明确反馈
- 让仓库约束更接近 executable policy，而不是口头约定
- 把人工判断留给真正需要判断的边界问题
