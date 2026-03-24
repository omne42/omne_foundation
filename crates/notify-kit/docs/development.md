# 开发

## 质量门禁

离线检查：

```bash
CARGO_NET_OFFLINE=true ./scripts/gate.sh
```

提交前严格检查（由 `githooks/pre-commit` 自动执行）：

```bash
./scripts/pre-commit-check.sh
```

常用命令：

```bash
cargo fmt --all --manifest-path Cargo.toml -- --check
cargo test --manifest-path Cargo.toml
```

## 目录结构

- `src/`：库实现
- `docs/`：mdBook 文档（本目录）
- `scripts/gate.sh`：格式化/编译门禁
- `scripts/pre-commit-check.sh`：提交前严格检查（clippy + 关键 lint）

## 文档维护

- 改动文档：直接编辑 `docs/*.md`
- 目录结构：编辑 `docs/SUMMARY.md`

## 本地预览（mdBook）

本目录使用 `SUMMARY.md` 作为目录。你可以用 mdBook 本地预览（含搜索）：

```bash
./scripts/docs.sh serve
```

传参示例（容器/远程访问）：

```bash
./scripts/docs.sh serve --hostname 0.0.0.0 --port 3000
```

编译 Rust 代码片段（mdBook `test`）：

```bash
./scripts/docs.sh test
```

首次使用需要安装：

```bash
cargo install mdbook --locked
```

## LLM 友好文档（llms.txt）

为了让 LLM/agent 更容易“看懂仓库文档”，我们提供了一个聚合文件：`llms.txt`。

更新后请重新生成：

```bash
./scripts/build-llms-txt.sh
```
