# GitHubCommentSink

`GitHubCommentSink` 会通过 GitHub REST API 在指定的 Issue / Pull Request 下创建一条评论（纯文本）。

## 构造

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{GitHubCommentConfig, GitHubCommentSink};

let cfg = GitHubCommentConfig::new("owner", "repo", 123, "ghp_xxx");
let sink = GitHubCommentSink::new(cfg)?;
# Ok(())
# }
```

## Token 权限

建议使用最小权限的 token：

- 对目标仓库具备 `issues:write`（PR 评论也走 issues API）

## 超时

`GitHubCommentConfig` 自带 HTTP timeout（默认 `2s`）。此外，`Hub` 也会对每个 sink 做兜底超时：

- 建议：`HubConfig.per_sink_timeout` ≥ `GitHubCommentConfig.timeout`

## 安全与隐私

- 固定请求 `https://api.github.com`，不会打印 token
- `Debug` 输出默认脱敏（不会泄露 token）
- 非 2xx 的响应不会包含 response body（避免泄露多余信息）
