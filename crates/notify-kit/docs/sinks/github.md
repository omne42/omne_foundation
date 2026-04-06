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

如果你要接 GitHub Enterprise 之类的自定义 API base，必须显式把 bearer token 允许发送到该 host：

```rust,no_run,edition2024
# extern crate notify_kit;
# fn main() -> notify_kit::Result<()> {
use notify_kit::{GitHubCommentConfig, GitHubCommentSink};

let cfg = GitHubCommentConfig::new("owner", "repo", 123, "ghp_xxx")
    .with_api_base("https://github.example.com/api/v3")
    .with_trusted_bearer_token_host("github.example.com");
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

- 默认请求 canonical `https://api.github.com`
- 自定义 `api_base` 只有在显式 `with_trusted_bearer_token_host(...)` 允许该 host 后才会携带 bearer token；同时仍然要求 `https`、URL 里不能带凭证、目标不能是 localhost / single-label / private IP，并继续做 DNS 公网 IP 校验
- `with_public_ip_check(false)` 不会放宽 bearer token 的 GitHub API 边界；带 token 的请求仍然会走共享的 public-IP pinning 路径
- `Debug` 输出默认脱敏（不会泄露 token，也不会暴露 `api_base` 的凭证/query/path 细节）
- 非 2xx 的响应不会包含 response body（避免泄露多余信息）
