use reqwest::RequestBuilder;
use reqwest::header::{ACCEPT, USER_AGENT};

use crate::error::{GitHubApiError, Result};

pub const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_API_VERSION: &str = "2022-11-28";
pub const DEFAULT_GITHUB_USER_AGENT: &str = "github-kit";
pub const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";

#[derive(Debug, Clone, Copy)]
pub struct GitHubApiRequestOptions<'a> {
    bearer_token: Option<&'a str>,
    user_agent: &'a str,
    api_version: &'a str,
}

impl<'a> Default for GitHubApiRequestOptions<'a> {
    fn default() -> Self {
        Self {
            bearer_token: None,
            user_agent: DEFAULT_GITHUB_USER_AGENT,
            api_version: DEFAULT_GITHUB_API_VERSION,
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

    pub(crate) fn has_bearer_token(&self) -> bool {
        self.bearer_token.is_some()
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
