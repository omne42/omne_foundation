use reqwest::RequestBuilder;
use reqwest::header::{ACCEPT, USER_AGENT};

use crate::error::{GitHubApiError, Result};

pub const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_API_VERSION: &str = "2022-11-28";
pub const DEFAULT_GITHUB_USER_AGENT: &str = "github-kit";
pub const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const CANONICAL_GITHUB_API_HOST: &str = "api.github.com";

#[derive(Debug, Clone, Copy)]
pub struct GitHubApiRequestOptions<'a> {
    bearer_token: Option<&'a str>,
    user_agent: &'a str,
    api_version: &'a str,
    allow_custom_bearer_api_base: bool,
}

impl<'a> Default for GitHubApiRequestOptions<'a> {
    fn default() -> Self {
        Self {
            bearer_token: None,
            user_agent: DEFAULT_GITHUB_USER_AGENT,
            api_version: DEFAULT_GITHUB_API_VERSION,
            allow_custom_bearer_api_base: false,
        }
    }
}

impl<'a> GitHubApiRequestOptions<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_bearer_token(mut self, bearer_token: Option<&'a str>) -> Self {
        self.bearer_token = bearer_token
            .map(str::trim)
            .filter(|value| !value.is_empty());
        self
    }

    #[must_use]
    pub fn with_user_agent(mut self, user_agent: &'a str) -> Self {
        let trimmed = user_agent.trim();
        if !trimmed.is_empty() {
            self.user_agent = trimmed;
        }
        self
    }

    #[must_use]
    pub fn with_api_version(mut self, api_version: &'a str) -> Self {
        let trimmed = api_version.trim();
        if !trimmed.is_empty() {
            self.api_version = trimmed;
        }
        self
    }

    /// Trust a non-canonical HTTPS GitHub API base for bearer-token requests.
    ///
    /// By default, bearer tokens are only sent to `https://api.github.com`.
    /// GitHub Enterprise or other custom API bases must opt in explicitly.
    #[must_use]
    pub fn with_allow_custom_bearer_api_base(mut self, allow: bool) -> Self {
        self.allow_custom_bearer_api_base = allow;
        self
    }

    pub(crate) fn has_bearer_token(&self) -> bool {
        self.bearer_token.is_some()
    }

    fn allows_custom_bearer_api_base(&self) -> bool {
        self.allow_custom_bearer_api_base
    }
}

pub fn apply_github_api_headers(
    mut request: RequestBuilder,
    options: GitHubApiRequestOptions<'_>,
) -> RequestBuilder {
    request = request
        .header(ACCEPT, GITHUB_API_ACCEPT)
        .header(USER_AGENT, options.user_agent)
        .header("X-GitHub-Api-Version", options.api_version);

    if let Some(token) = options.bearer_token {
        request = request.bearer_auth(token);
    }

    request
}

pub fn build_github_api_url<I, S>(api_base: &str, segments: I) -> Result<reqwest::Url>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let trimmed = api_base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(GitHubApiError::NoApiBaseConfigured);
    }

    let mut url = reqwest::Url::parse(trimmed).map_err(|err| GitHubApiError::InvalidApiBase {
        details: err.to_string(),
    })?;
    let mut path_segments =
        url.path_segments_mut()
            .map_err(|_| GitHubApiError::InvalidApiBase {
                details: "base URL cannot accept path segments".to_string(),
            })?;
    for segment in segments {
        path_segments.push(segment.as_ref());
    }
    drop(path_segments);
    Ok(url)
}

pub fn validate_github_api_request_url(
    url: &reqwest::Url,
    options: GitHubApiRequestOptions<'_>,
) -> Result<()> {
    if !options.has_bearer_token() {
        return Ok(());
    }

    if url.scheme() != "https" {
        return Err(GitHubApiError::InvalidApiBase {
            details: "bearer token requires an https github api base".to_string(),
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(GitHubApiError::InvalidApiBase {
            details: "bearer token requires a github api base without credentials".to_string(),
        });
    }

    let Some(host) = url.host_str() else {
        return Err(GitHubApiError::InvalidApiBase {
            details: "github api base must include a host".to_string(),
        });
    };

    if !options.allows_custom_bearer_api_base()
        && !host.eq_ignore_ascii_case(CANONICAL_GITHUB_API_HOST)
    {
        return Err(GitHubApiError::InvalidApiBase {
            details: "bearer token requires the canonical GitHub API base `https://api.github.com` unless custom bases are explicitly trusted".to_string(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_custom_public_bearer_api_base_without_explicit_opt_in() {
        let url = reqwest::Url::parse("https://github.example.com/api/v3/repos/omne42/repo")
            .expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .expect_err("custom base should be rejected");

        let message = err.to_string();
        assert!(message.contains("canonical GitHub API base"), "{message}");
    }

    #[test]
    fn allows_custom_public_bearer_api_base_after_explicit_opt_in() {
        let url = reqwest::Url::parse("https://github.example.com/api/v3/repos/omne42/repo")
            .expect("url");

        validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_allow_custom_bearer_api_base(true),
        )
        .expect("explicitly trusted custom base should be allowed");
    }
}
