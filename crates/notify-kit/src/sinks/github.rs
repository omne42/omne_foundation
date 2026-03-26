use std::time::Duration;

use crate::Event;
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::{BoxFuture, Sink};
use http_kit::{build_http_client, ensure_http_success, redact_url, send_reqwest};

const GITHUB_API_BASE: &str = "https://api.github.com";

#[non_exhaustive]
#[derive(Clone)]
pub struct GitHubCommentConfig {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub token: String,
    pub timeout: Duration,
    pub max_chars: usize,
}

impl std::fmt::Debug for GitHubCommentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubCommentConfig")
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("issue_number", &self.issue_number)
            .field("token", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .finish()
    }
}

impl GitHubCommentConfig {
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        issue_number: u64,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            issue_number,
            token: token.into(),
            timeout: Duration::from_secs(2),
            max_chars: 65000,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }
}

pub struct GitHubCommentSink {
    api_url: reqwest::Url,
    owner: String,
    repo: String,
    issue_number: u64,
    token: String,
    client: reqwest::Client,
    max_chars: usize,
}

impl std::fmt::Debug for GitHubCommentSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubCommentSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("issue_number", &self.issue_number)
            .field("token", &"<redacted>")
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}

impl GitHubCommentSink {
    pub fn new(config: GitHubCommentConfig) -> crate::Result<Self> {
        let owner = normalize_github_identifier("owner", &config.owner)?;
        let repo = normalize_github_identifier("repo", &config.repo)?;
        if config.issue_number == 0 {
            return Err(anyhow::anyhow!("github issue_number must be > 0").into());
        }
        let token = config.token.trim();
        if token.is_empty() {
            return Err(anyhow::anyhow!("github token must not be empty").into());
        }

        let api_url = build_issue_comment_url(owner, repo, config.issue_number)?;
        let client = build_http_client(config.timeout)?;

        Ok(Self {
            api_url,
            owner: owner.to_string(),
            repo: repo.to_string(),
            issue_number: config.issue_number,
            token: token.to_string(),
            client,
            max_chars: config.max_chars,
        })
    }

    fn build_payload(event: &Event, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        serde_json::json!({ "body": text })
    }
}

fn normalize_github_identifier<'a>(kind: &'static str, value: &'a str) -> crate::Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(anyhow::anyhow!("github {kind} must not be empty").into());
    }
    if value.contains('/') {
        return Err(anyhow::anyhow!("github {kind} must not contain '/'").into());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(anyhow::anyhow!("github {kind} contains invalid characters").into());
    }
    Ok(value)
}

fn build_issue_comment_url(
    owner: &str,
    repo: &str,
    issue_number: u64,
) -> crate::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(GITHUB_API_BASE)
        .map_err(|err| anyhow::anyhow!("invalid github api base url: {err}"))?;
    let issue_segment = issue_number.to_string();
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid github api base url"))?
        .extend([
            "repos",
            owner,
            repo,
            "issues",
            issue_segment.as_str(),
            "comments",
        ]);
    Ok(url)
}

impl Sink for GitHubCommentSink {
    fn name(&self) -> &'static str {
        "github"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let payload = Self::build_payload(event, self.max_chars);

            let resp = send_reqwest(
                self.client
                    .post(self.api_url.as_str())
                    .header("Accept", "application/vnd.github+json")
                    .header("User-Agent", "notify-kit")
                    .header("X-GitHub-Api-Version", "2022-11-28")
                    .bearer_auth(&self.token)
                    .json(&payload),
                "github comment",
            )
            .await?;
            Ok(ensure_http_success(resp, "github comment").await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = GitHubCommentSink::build_payload(&event, 65000);
        let text = payload["body"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_empty_owner() {
        let cfg = GitHubCommentConfig::new("", "repo", 1, "tok");
        let err = GitHubCommentSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("owner"), "{err:#}");
    }

    #[test]
    fn rejects_slash_in_owner() {
        let cfg = GitHubCommentConfig::new("a/b", "repo", 1, "tok");
        let err = GitHubCommentSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("contain '/'"), "{err:#}");
    }

    #[test]
    fn rejects_issue_number_zero() {
        let cfg = GitHubCommentConfig::new("owner", "repo", 0, "tok");
        let err = GitHubCommentSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("issue_number"), "{err:#}");
    }

    #[test]
    fn debug_redacts_token() {
        let cfg = GitHubCommentConfig::new("owner", "repo", 1, "tok_secret");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("tok_secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = GitHubCommentSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("tok_secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("api.github.com"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn trims_owner_repo_and_token() {
        let cfg = GitHubCommentConfig::new(" owner ", " repo ", 1, " tok ");
        let sink = GitHubCommentSink::new(cfg).expect("build sink");
        assert_eq!(sink.owner, "owner");
        assert_eq!(sink.repo, "repo");
        assert_eq!(sink.token, "tok");
    }
}
