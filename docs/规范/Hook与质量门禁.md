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
- workspace 内部 crate 依赖方向检查
- workspace 发布契约回归检查
- workspace package 级 Rust 格式检查（`cargo fmt` + package 列表 + `--check`）
- `cargo check --workspace --all-targets --all-features`，但会排除硬件 opt-in feature crate 并单独用默认 feature 检查它们
- `cargo test --workspace --all-features`，但会排除硬件 opt-in feature crate 并单独用默认 feature 测试它们

也就是说，提交前默认要满足文档结构、依赖方向、发布契约、格式、编译和测试这六类本地门禁。

`speech-whisper-kit` 的 `metal` / `cuda` feature 属于硬件 opt-in feature。普通 `local` / `ci` 门禁只检查它的默认 CPU feature，避免在没有对应 GPU SDK 的 CI runner 上把硬件后端误当成必需基线。

`scripts/workspace_check/` 是这里的共享实现；`scripts/check-workspace.sh` 只是保留给手动执行和 CI 复用的薄入口。

如果当前环境缺少 `cargo`，这些需要 Rust 工具链的 gate 会在入口阶段直接报出清晰错误，说明缺少的命令和对应检查用途；同时会优先使用 `PATH` 里的 `cargo`，必要时回退到 `~/.cargo/bin/cargo`，避免把环境问题退化成 `traceback` 或裸 `cargo: not found`。

### `review-root` gate

针对 root review 已收口的问题，workspace check 还保留一个更窄的回归入口：

- `cargo check -p mcp-jsonrpc`
- `cargo check -p notify-kit`
- `cargo check -p policy-meta`
- `cargo check -p mcp-kit`
- `cargo test -p http-kit`
- `cargo test -p github-kit`

可以单独执行：

```bash
scripts/check-workspace.sh review-root
```

这个入口的目的不是替代 `local` / `ci`，而是把已经在 review 中暴露过、且需要持续防回归的问题集中成一组更快的定向检查。

### dependency direction gate

`scripts/workspace_check/` 现在还会机械检查 workspace 内部 crate 依赖方向。

当前做法是维护一份与 [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) 对齐的稳定 allowlist，只允许声明过的内部依赖组合继续存在；同时也拒绝 allowlist 比真实依赖更宽的“陈旧白名单”。一旦某个 crate 新增了未记录的 workspace 依赖，或文档/allowlist 仍保留了已经删除的内部依赖，`local` / `ci` 和单独的：

```bash
scripts/check-workspace.sh dependency-direction
```

都会直接失败。

这条 gate 的目的不是阻止正常演进，而是把“内部依赖发生变化”变成一个显式动作：

- 先决定边界是否合理
- 再同步更新 `ARCHITECTURE.md`
- 再同步收紧或放宽 allowlist
- 然后让 gate 接受新的依赖方向

### publish contract gate

`scripts/workspace_check/` 现在也会机械检查“改动中的 crate 是否还维持一致的发布契约”。

当前规则分两类：

- repo-wide 检查（扫描整个 workspace）：
- workspace 包不能通过 `path = "../..."` 之类的仓库外路径依赖逃出当前 repo root；像 `omne-runtime` 这样的跨仓 foundation/runtime crate 必须改用 canonical git source pin
- 如果某个 crate 已经声明 `publish = false`，它自己的 `README.md` 也不能再把 crates.io 安装写成当前可直接使用的主契约
- changed-manifest 检查（只针对本次改动里的 `crates/*/Cargo.toml`）：
- workspace 包如果声明了 path 依赖或 git-sourced foundation/runtime 依赖，manifest 里也必须同时写显式 `version`，避免 `cargo package` 导出时把 semver 边降成 `*`
- 如果某个 crate 的普通依赖或 build-dependencies（含 target-specific 表）引用了 workspace 内已经声明 `publish = false` 的 crate，它自己也必须显式 `publish = false`
- 如果某个 crate 仍直接依赖 git-sourced foundation/runtime crate，它自己不能继续保留“默认可走 crates.io 发布”的隐式契约，必须显式 `publish = false` 或移除这条 git 依赖
- 任一规则不满足都会让 gate 直接失败，避免 manifest 继续暗示“当前可单独走 crates.io 发布”或“当前仍与 sibling workspace 隐式绑死”，直到 `cargo package` / `cargo metadata` / 实际跨仓复用时才暴露真实边界

可以单独执行：

```bash
scripts/check-workspace.sh publish-contract
```

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
- `crates/notify-kit/bots/_shared/*.test.mjs` / `*.test.js` 的 `node --test`

如果本机没有 `node`，bot syntax check 和 `_shared` Node tests 都会被跳过并输出提示。

## `workspace_check` 的其他模式

除了 `local`，共享检查核心还支持：

- `ci`
- `docs-system`
- `dependency-direction`
- `publish-contract`
- `asset-checks [all|policy-meta|mcp-kit|notify-kit]`
- `review-root`
- `secret-kit-target <target-triple>`

其中：

- `ci` 在 `local` 基础上增加 `clippy` 和全量 asset checks；`clippy` 对硬件 opt-in feature crate 同样只跑默认 feature
- `docs-system` 只运行文档系统入口与链接约束检查
- `dependency-direction` 只运行 workspace 内部 crate 依赖方向 gate
- `publish-contract` 只运行 workspace 发布契约 gate
- `review-root` 用于把 root review 里已经归并过的高价值结论，在最新 `main` 基线上做一次快速机械复核
- `secret-kit-target` 用于检查 `secret-kit` 的特定 target 编译

## review 快速复核

当 root review 产出多个审查文件，或者结论来自较早的工作树快照时，不应直接把这些结果当成当前 `main` 的事实。

推荐顺序是：

1. 先合并重复项，把同一类编译断链、依赖缺失、测试回归收敛成一份问题清单。
2. 再切到最新 `main` 基线复核，避免把已经修复的问题重复计入当前结论。
3. 最后再决定是否需要拆分修复分支、补测试或升级 gate。

针对本仓库 root review 的常见高风险项，`workspace_check` 提供了一个收敛入口：

```bash
scripts/check-workspace.sh review-root
```

这个模式的用途是：

- 快速验证 review 中最容易阻断 workspace 的编译与测试项是否仍然存在
- 给 review 去重后的结论提供最新基线上的机械证据
- 在进入更重的 `local` / `ci` 全量门禁前，先做一次低成本收口

它的边界也需要明确：

- 它不是 `local` 或 `ci` 的替代品
- 它只覆盖预设的 root review 关键路径，不负责发现新的广义回归
- 如果 `review-root` 已经通过，仍然要继续跑正常门禁，才能作为可合并结论

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
