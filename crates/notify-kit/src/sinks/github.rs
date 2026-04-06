use std::time::Duration;

use crate::Event;
use crate::SecretString;
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::{BoxFuture, Sink};
use github_kit::{
    DEFAULT_GITHUB_API_BASE, GitHubApiRequestOptions, apply_github_api_headers,
    build_github_api_url, validate_github_api_request_url, validate_github_api_request_url_dns,
};
use http_kit::{
    HttpClientOptions, HttpClientProfile, build_http_client_profile, ensure_http_success,
    redact_url, send_reqwest,
};

#[non_exhaustive]
#[derive(Clone)]
pub struct GitHubCommentConfig {
    pub api_base: String,
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub token: SecretString,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
    pub trusted_bearer_token_hosts: Vec<String>,
}

impl std::fmt::Debug for GitHubCommentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubCommentConfig")
            .field("api_base", &http_kit::redact_url_str(&self.api_base))
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("issue_number", &self.issue_number)
            .field("token", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field(
                "trusted_bearer_token_hosts",
                &self.trusted_bearer_token_hosts,
            )
            .finish()
    }
}

impl GitHubCommentConfig {
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        issue_number: u64,
        token: impl Into<SecretString>,
    ) -> Self {
        Self {
            api_base: DEFAULT_GITHUB_API_BASE.to_string(),
            owner: owner.into(),
            repo: repo.into(),
            issue_number,
            token: token.into(),
            timeout: Duration::from_secs(2),
            max_chars: 65000,
            enforce_public_ip: true,
            trusted_bearer_token_hosts: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }

    #[must_use]
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    #[must_use]
    pub fn with_public_ip_check(mut self, enforce_public_ip: bool) -> Self {
        self.enforce_public_ip = enforce_public_ip;
        self
    }

    #[must_use]
    pub fn with_trusted_bearer_token_host(mut self, host: impl Into<String>) -> Self {
        let host = host.into().trim().trim_end_matches('.').to_string();
        if !host.is_empty() {
            self.trusted_bearer_token_hosts.push(host);
        }
        self
    }
}

pub struct GitHubCommentSink {
    api_url: reqwest::Url,
    owner: String,
    repo: String,
    issue_number: u64,
    token: SecretString,
    http: HttpClientProfile,
    max_chars: usize,
    enforce_public_ip: bool,
    trusted_bearer_token_hosts: Vec<String>,
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
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field(
                "trusted_bearer_token_hosts",
                &self.trusted_bearer_token_hosts,
            )
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
        let token = normalize_secret(config.token, "token")?;

        let api_url = build_issue_comment_url(&config.api_base, owner, repo, config.issue_number)?;
        let trusted_hosts = config
            .trusted_bearer_token_hosts
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        validate_github_api_request_url(
            &api_url,
            GitHubApiRequestOptions::new()
                .with_user_agent("notify-kit")
                .with_bearer_token(Some(token.expose_secret()))
                .with_trusted_bearer_token_hosts(trusted_hosts.as_slice()),
        )
        .map_err(anyhow::Error::new)?;
        let http = build_http_client_profile(&HttpClientOptions {
            timeout: Some(config.timeout),
            ..Default::default()
        })?;

        Ok(Self {
            api_url,
            owner: owner.to_string(),
            repo: repo.to_string(),
            issue_number: config.issue_number,
            token,
            http,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
            trusted_bearer_token_hosts: config.trusted_bearer_token_hosts,
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
    api_base: &str,
    owner: &str,
    repo: &str,
    issue_number: u64,
) -> crate::Result<reqwest::Url> {
    let issue_segment = issue_number.to_string();
    build_github_api_url(
        api_base,
        [
            "repos",
            owner,
            repo,
            "issues",
            issue_segment.as_str(),
            "comments",
        ],
    )
    .map_err(|err| anyhow::Error::new(err).into())
}

impl Sink for GitHubCommentSink {
    fn name(&self) -> &'static str {
        "github"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let trusted_hosts = self
                .trusted_bearer_token_hosts
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let request_options = GitHubApiRequestOptions::new()
                .with_user_agent("notify-kit")
                .with_bearer_token(Some(self.token.expose_secret()))
                .with_trusted_bearer_token_hosts(trusted_hosts.as_slice());
            validate_github_api_request_url_dns(&self.api_url, request_options)
                .await
                .map_err(anyhow::Error::new)?;
            let client = self
                .http
                .select_for_url(
                    &self.api_url,
                    self.enforce_public_ip || request_options.requires_public_ip_pinning(),
                )
                .await?;
            let payload = Self::build_payload(event, self.max_chars);
            let request = apply_github_api_headers(
                client.post(self.api_url.as_str()).json(&payload),
                request_options,
            )
            .map_err(anyhow::Error::new)?;

            let resp = send_reqwest(request, "github comment").await?;
            Ok(ensure_http_success(resp, "github comment").await?)
        })
    }
}

fn normalize_secret(secret: SecretString, field: &str) -> crate::Result<SecretString> {
    let secret = secret.expose_secret().trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("github {field} must not be empty").into());
    }
    Ok(SecretString::new(secret))
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
    fn debug_redacts_api_base_credentials_and_query() {
        let cfg = GitHubCommentConfig::new("owner", "repo", 1, "tok_secret")
            .with_api_base("https://user:pass@github.example.com/api/v3?token=top-secret");

        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("user"), "{cfg_dbg}");
        assert!(!cfg_dbg.contains("pass"), "{cfg_dbg}");
        assert!(!cfg_dbg.contains("token=top-secret"), "{cfg_dbg}");
        assert!(!cfg_dbg.contains("/api/v3"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("github.example.com"), "{cfg_dbg}");
    }

    #[test]
    fn trims_owner_repo_and_token() {
        let cfg = GitHubCommentConfig::new(" owner ", " repo ", 1, " tok ");
        let sink = GitHubCommentSink::new(cfg).expect("build sink");
        assert_eq!(sink.owner, "owner");
        assert_eq!(sink.repo, "repo");
        assert_eq!(sink.token.expose_secret(), "tok");
    }

    #[test]
    fn preserves_custom_api_base_path() {
        let cfg = GitHubCommentConfig::new("owner", "repo", 1, "tok")
            .with_api_base("https://github.example.com/api/v3/")
            .with_trusted_bearer_token_host("github.example.com");
        let sink = GitHubCommentSink::new(cfg).expect("build sink");
        assert_eq!(
            sink.api_url.as_str(),
            "https://github.example.com/api/v3/repos/owner/repo/issues/1/comments"
        );
    }

    #[test]
    fn rejects_untrusted_custom_api_base() {
        let cfg = GitHubCommentConfig::new("owner", "repo", 1, "tok")
            .with_api_base("https://github.example.com/api/v3/");
        let err = GitHubCommentSink::new(cfg).expect_err("custom host should require opt-in");
        assert!(err.to_string().contains("explicit trusted host allowlist"));
    }
}
