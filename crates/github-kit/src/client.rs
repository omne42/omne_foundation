use http_kit::{
    UntrustedOutboundPolicy, validate_untrusted_outbound_url, validate_untrusted_outbound_url_dns,
};
use reqwest::Request;
use reqwest::RequestBuilder;
use reqwest::header::{ACCEPT, HeaderValue, USER_AGENT};

use crate::error::{GitHubApiError, Result};

pub const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_API_VERSION: &str = "2022-11-28";
pub const DEFAULT_GITHUB_USER_AGENT: &str = "github-kit";
pub const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
pub const CANONICAL_GITHUB_API_HOST: &str = "api.github.com";

#[derive(Debug, Clone, Copy)]
pub struct GitHubApiRequestOptions<'a> {
    bearer_token: Option<&'a str>,
    user_agent: &'a str,
    api_version: &'a str,
    trusted_bearer_token_hosts: &'a [&'a str],
}

impl<'a> Default for GitHubApiRequestOptions<'a> {
    fn default() -> Self {
        Self {
            bearer_token: None,
            user_agent: DEFAULT_GITHUB_USER_AGENT,
            api_version: DEFAULT_GITHUB_API_VERSION,
            trusted_bearer_token_hosts: &[],
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

    #[must_use]
    pub fn with_trusted_bearer_token_hosts(
        mut self,
        trusted_bearer_token_hosts: &'a [&'a str],
    ) -> Self {
        self.trusted_bearer_token_hosts = trusted_bearer_token_hosts;
        self
    }

    pub(crate) fn has_bearer_token(&self) -> bool {
        self.bearer_token.is_some()
    }

    #[must_use]
    pub fn requires_public_ip_pinning(&self) -> bool {
        self.has_bearer_token()
    }

    pub(crate) fn bearer_token_host_is_trusted(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(CANONICAL_GITHUB_API_HOST)
            || self
                .trusted_bearer_token_hosts
                .iter()
                .map(|candidate| candidate.trim().trim_end_matches('.'))
                .filter(|candidate| !candidate.is_empty())
                .any(|candidate| host.eq_ignore_ascii_case(candidate))
    }
}

fn apply_github_api_headers_unchecked(
    mut request: Request,
    options: GitHubApiRequestOptions<'_>,
) -> Result<Request> {
    request.headers_mut().insert(
        ACCEPT,
        reqwest::header::HeaderValue::from_static(GITHUB_API_ACCEPT),
    );
    request.headers_mut().insert(
        USER_AGENT,
        header_value_from_str(USER_AGENT.as_str(), options.user_agent)?,
    );
    request.headers_mut().insert(
        reqwest::header::HeaderName::from_static("x-github-api-version"),
        header_value_from_str("x-github-api-version", options.api_version)?,
    );

    if let Some(token) = options.bearer_token {
        let bearer = format!("Bearer {token}");
        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            header_value_from_str(reqwest::header::AUTHORIZATION.as_str(), &bearer)?,
        );
    }

    Ok(request)
}

fn header_value_from_str(header: &'static str, value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|err| GitHubApiError::InvalidHeaderValue {
        header,
        details: err.to_string(),
    })
}

pub fn apply_github_api_headers(
    request: RequestBuilder,
    options: GitHubApiRequestOptions<'_>,
) -> Result<RequestBuilder> {
    let (client, request) = request.build_split();
    let request = request.map_err(|err| GitHubApiError::InvalidApiBase {
        details: format!("invalid github api request builder: {err}"),
    })?;
    validate_github_api_request_url(request.url(), options)?;
    Ok(RequestBuilder::from_parts(
        client,
        apply_github_api_headers_unchecked(request, options)?,
    ))
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
    if url.query().is_some() || url.fragment().is_some() {
        return Err(GitHubApiError::InvalidApiBase {
            details:
                "bearer token requires a github api base without query parameters or fragments"
                    .to_string(),
        });
    }

    let host = url
        .host_str()
        .ok_or_else(|| GitHubApiError::InvalidApiBase {
            details: "bearer token requires a github api base with a host".to_string(),
        })?;
    if !options.bearer_token_host_is_trusted(host) {
        return Err(GitHubApiError::InvalidApiBase {
            details: format!(
                "bearer token requires the canonical GitHub API base (`https://api.github.com`) or an explicit trusted host allowlist: {host}"
            ),
        });
    }

    validate_untrusted_outbound_url(&UntrustedOutboundPolicy::default(), url).map_err(|err| {
        GitHubApiError::InvalidApiBase {
            details: format!("bearer token target is not allowed: {err}"),
        }
    })?;

    Ok(())
}

pub async fn validate_github_api_request_url_dns(
    url: &reqwest::Url,
    options: GitHubApiRequestOptions<'_>,
) -> Result<()> {
    validate_github_api_request_url(url, options)?;
    if !options.requires_public_ip_pinning() {
        return Ok(());
    }

    validate_untrusted_outbound_url_dns(&UntrustedOutboundPolicy::default(), url)
        .await
        .map_err(|err| GitHubApiError::InvalidApiBase {
            details: format!("bearer token target is not allowed: {err}"),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{AUTHORIZATION, HeaderValue};

    #[test]
    fn allows_explicitly_trusted_public_custom_host() {
        let url = reqwest::Url::parse("https://github.example.com/api/v3/repos/omne42/repo")
            .expect("url");

        validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.com"]),
        )
        .expect("trusted public host should be allowed");
    }

    #[test]
    fn rejects_private_ip_even_if_explicitly_allowlisted() {
        let url = reqwest::Url::parse("https://127.0.0.1/api/v3/repos/omne42/repo").expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["127.0.0.1"]),
        )
        .expect_err("private ip should still be rejected");

        let message = err.to_string();
        assert!(message.contains("not allowed"), "{message}");
        assert!(message.contains("127.0.0.1"), "{message}");
    }

    #[tokio::test]
    async fn dns_validation_rejects_unresolvable_custom_bearer_api_base_after_opt_in() {
        let url = reqwest::Url::parse("https://github.example.invalid/api/v3/repos/omne42/repo")
            .expect("url");

        let err = validate_github_api_request_url_dns(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.invalid"]),
        )
        .await
        .expect_err("unresolvable target should fail closed");

        let message = err.to_string();
        assert!(message.contains("dns lookup"), "{message}");
    }

    #[test]
    fn rejects_trusted_custom_host_with_url_credentials() {
        let url =
            reqwest::Url::parse("https://user:pass@github.example.com/api/v3/repos/omne42/repo")
                .expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.com"]),
        )
        .expect_err("credentialed bearer target should fail closed");

        let message = err.to_string();
        assert!(message.contains("without credentials"), "{message}");
    }

    #[test]
    fn rejects_trusted_custom_host_over_http() {
        let url =
            reqwest::Url::parse("http://github.example.com/api/v3/repos/omne42/repo").expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.com"]),
        )
        .expect_err("non-https bearer target should fail closed");

        let message = err.to_string();
        assert!(message.contains("requires an https"), "{message}");
    }

    #[test]
    fn rejects_canonical_host_with_query() {
        let url = reqwest::Url::parse("https://api.github.com/repos/omne42/repo?x=1").expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .expect_err("query-bearing bearer target should fail closed");

        let message = err.to_string();
        assert!(
            message.contains("without query parameters or fragments"),
            "{message}"
        );
    }

    #[test]
    fn rejects_canonical_host_with_fragment() {
        let url =
            reqwest::Url::parse("https://api.github.com/repos/omne42/repo#frag").expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .expect_err("fragment-bearing bearer target should fail closed");

        let message = err.to_string();
        assert!(
            message.contains("without query parameters or fragments"),
            "{message}"
        );
    }

    #[test]
    fn rejects_trusted_custom_host_with_query() {
        let url = reqwest::Url::parse("https://github.example.com/api/v3/repos/omne42/repo?x=1")
            .expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.com"]),
        )
        .expect_err("query-bearing trusted custom bearer target should fail closed");

        let message = err.to_string();
        assert!(
            message.contains("without query parameters or fragments"),
            "{message}"
        );
    }

    #[test]
    fn rejects_trusted_custom_host_with_fragment() {
        let url = reqwest::Url::parse("https://github.example.com/api/v3/repos/omne42/repo#frag")
            .expect("url");

        let err = validate_github_api_request_url(
            &url,
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.com"]),
        )
        .expect_err("fragment-bearing trusted custom bearer target should fail closed");

        let message = err.to_string();
        assert!(
            message.contains("without query parameters or fragments"),
            "{message}"
        );
    }

    #[test]
    fn apply_headers_rejects_untrusted_bearer_target() {
        let client = reqwest::Client::new();

        let err = apply_github_api_headers(
            client.get("https://example.invalid/api/v3/repos/omne42/repo"),
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .expect_err("untrusted bearer target should fail");

        let message = err.to_string();
        assert!(message.contains("canonical GitHub API base"), "{message}");
    }

    #[test]
    fn apply_headers_attaches_bearer_for_trusted_target() {
        let client = reqwest::Client::new();

        let request = apply_github_api_headers(
            client.get("https://api.github.com/repos/omne42/repo"),
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .expect("canonical target should be allowed")
        .build()
        .expect("request");

        assert_eq!(
            request.headers().get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer secret-token"))
        );
    }

    #[test]
    fn apply_headers_rejects_invalid_user_agent_header_value() {
        let client = reqwest::Client::new();

        let err = apply_github_api_headers(
            client.get("https://api.github.com/repos/omne42/repo"),
            GitHubApiRequestOptions::new().with_user_agent("github-kit\nbroken"),
        )
        .expect_err("invalid user-agent should return a typed error");

        match err {
            GitHubApiError::InvalidHeaderValue { header, details } => {
                assert_eq!(header, "user-agent");
                assert!(
                    !details.is_empty(),
                    "header error should include parser details"
                );
            }
            other => panic!("expected InvalidHeaderValue, got {other:?}"),
        }
    }

    #[test]
    fn apply_headers_rejects_invalid_api_version_header_value() {
        let client = reqwest::Client::new();

        let err = apply_github_api_headers(
            client.get("https://api.github.com/repos/omne42/repo"),
            GitHubApiRequestOptions::new().with_api_version("2022-11-28\r\nbroken"),
        )
        .expect_err("invalid api version should return a typed error");

        match err {
            GitHubApiError::InvalidHeaderValue { header, details } => {
                assert_eq!(header, "x-github-api-version");
                assert!(
                    !details.is_empty(),
                    "header error should include parser details"
                );
            }
            other => panic!("expected InvalidHeaderValue, got {other:?}"),
        }
    }

    #[test]
    fn apply_headers_rejects_invalid_authorization_header_value() {
        let client = reqwest::Client::new();

        let err = apply_github_api_headers(
            client.get("https://api.github.com/repos/omne42/repo"),
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret\nbroken")),
        )
        .expect_err("invalid bearer token should return a typed error");

        match err {
            GitHubApiError::InvalidHeaderValue { header, details } => {
                assert_eq!(header, "authorization");
                assert!(
                    !details.is_empty(),
                    "header error should include parser details"
                );
            }
            other => panic!("expected InvalidHeaderValue, got {other:?}"),
        }
    }
}
