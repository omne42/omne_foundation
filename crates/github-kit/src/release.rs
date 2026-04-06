use http_kit::{
    UntrustedOutboundPolicy, read_json_body_after_http_success_limited, redact_url_for_error,
    redact_url_str, send_reqwest, validate_untrusted_outbound_url_dns,
};
use serde::Deserialize;

use crate::client::{
    GitHubApiRequestOptions, apply_github_api_headers, build_github_api_url,
    validate_github_api_request_url,
};
use crate::error::{GitHubApiError, Result};

const GITHUB_LATEST_RELEASE_MAX_JSON_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub digest: Option<String>,
}

pub async fn fetch_latest_release<S: AsRef<str>>(
    client: &reqwest::Client,
    api_bases: &[S],
    repo: &str,
    options: GitHubApiRequestOptions<'_>,
) -> Result<GitHubRelease> {
    let (owner, name) = normalize_repository(repo)?;
    let repo = format!("{owner}/{name}");
    let mut errors = Vec::new();
    let mut attempted = false;

    for base in api_bases {
        let trimmed = base.as_ref().trim().trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        attempted = true;

        let url = match build_github_api_url(trimmed, ["repos", owner, name, "releases", "latest"])
        {
            Ok(url) => url,
            Err(err) => {
                errors.push(format!("{} -> {err}", redact_url_str(trimmed)));
                continue;
            }
        };
        let redacted_url = redact_url_for_error(&url);
        if let Err(err) = validate_api_url_for_bearer_token(&url, options).await {
            errors.push(format!("{redacted_url} -> {err}"));
            continue;
        }

        let request = match apply_github_api_headers(client.get(url.clone()), &url, options) {
            Ok(request) => request,
            Err(err) => {
                errors.push(format!("{redacted_url} -> {err}"));
                continue;
            }
        };

        let response = match send_reqwest(request, "github latest release").await {
            Ok(response) => response,
            Err(err) => {
                errors.push(format!("{redacted_url} -> {err}"));
                continue;
            }
        };

        let json = match read_json_body_after_http_success_limited(
            response,
            "github latest release",
            GITHUB_LATEST_RELEASE_MAX_JSON_BYTES,
        )
        .await
        {
            Ok(json) => json,
            Err(err) => {
                errors.push(format!("{redacted_url} -> {err}"));
                continue;
            }
        };

        match serde_json::from_value::<GitHubRelease>(json) {
            Ok(release) => return Ok(release),
            Err(err) => errors.push(format!("{redacted_url} -> invalid json: {err}")),
        }
    }

    if !attempted {
        return Err(GitHubApiError::NoApiBaseConfigured);
    }

    Err(GitHubApiError::LatestReleaseFetchFailed {
        repo,
        details: errors.join(" | "),
    })
}

fn normalize_repository(repo: &str) -> Result<(&str, &str)> {
    let trimmed = repo.trim();
    let Some((owner, name)) = trimmed.split_once('/') else {
        return Err(GitHubApiError::InvalidRepository {
            repo: trimmed.to_string(),
        });
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || owner.chars().any(char::is_whitespace)
        || name.chars().any(char::is_whitespace)
    {
        return Err(GitHubApiError::InvalidRepository {
            repo: trimmed.to_string(),
        });
    }
    Ok((owner, name))
}

async fn validate_api_url_for_bearer_token(
    url: &reqwest::Url,
    options: GitHubApiRequestOptions<'_>,
) -> Result<()> {
    validate_github_api_request_url(url, options)?;
    if !options.has_bearer_token() {
        return Ok(());
    }

    validate_untrusted_outbound_url_dns(&UntrustedOutboundPolicy::default(), url)
        .await
        .map_err(|err| GitHubApiError::InvalidApiBase {
            details: format!("bearer token target is not allowed: {err}"),
        })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn reject_invalid_repository_identifier() {
        let err = normalize_repository("invalid").expect_err("invalid repo should fail");
        assert!(err.to_string().contains("owner/repo"));
    }

    #[test]
    fn build_api_url_preserves_base_path_prefix() {
        let url = build_github_api_url(
            "https://example.invalid/api/v3/",
            ["repos", "omne42", "repo", "issues", "1", "comments"],
        )
        .expect("url");

        assert_eq!(
            url.as_str(),
            "https://example.invalid/api/v3/repos/omne42/repo/issues/1/comments"
        );
    }

    #[tokio::test]
    async fn fetch_latest_release_falls_back_across_api_bases() {
        let responses = vec![
            MockHttpResponse {
                expected_path: "/api-fail/repos/cli/cli/releases/latest".to_string(),
                expected_headers: vec![
                    ("accept".to_string(), "application/vnd.github+json".to_string()),
                    ("user-agent".to_string(), "toolchain-installer".to_string()),
                    ("x-github-api-version".to_string(), "2022-11-28".to_string()),
                ],
                status_line: "HTTP/1.1 500 Internal Server Error",
                body: "{\"message\":\"try next\"}".to_string(),
            },
            MockHttpResponse {
                expected_path: "/api-ok/repos/cli/cli/releases/latest".to_string(),
                expected_headers: vec![
                    ("accept".to_string(), "application/vnd.github+json".to_string()),
                    ("user-agent".to_string(), "toolchain-installer".to_string()),
                    ("x-github-api-version".to_string(), "2022-11-28".to_string()),
                ],
                status_line: "HTTP/1.1 200 OK",
                body: r#"{"tag_name":"v2.0.0","assets":[{"name":"asset.tar.gz","browser_download_url":"https://example.invalid/asset.tar.gz","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#.to_string(),
            },
        ];
        let (base, handle) = spawn_mock_server(responses);
        let client = reqwest::Client::new();

        let release = fetch_latest_release(
            &client,
            &[format!("{base}/api-fail"), format!("{base}/api-ok")],
            "cli/cli",
            GitHubApiRequestOptions::new().with_user_agent("toolchain-installer"),
        )
        .await
        .expect("release");

        assert_eq!(release.tag_name, "v2.0.0");
        assert_eq!(release.assets[0].name, "asset.tar.gz");
        handle.join().expect("mock server thread");
    }

    #[tokio::test]
    async fn fetch_latest_release_accepts_large_but_bounded_json_payloads() {
        let long_asset_name = "asset".repeat(4_300);
        let body = format!(
            "{{\"tag_name\":\"v2.0.0\",\"assets\":[{{\"name\":\"{long_asset_name}\",\"browser_download_url\":\"https://example.invalid/asset.tar.gz\",\"digest\":null}}]}}"
        );
        assert!(
            body.len() > 16 * 1024,
            "test body should exceed old 16 KiB cap"
        );
        assert!(
            body.len() < GITHUB_LATEST_RELEASE_MAX_JSON_BYTES,
            "test body should remain within github release cap"
        );

        let responses = vec![MockHttpResponse {
            expected_path: "/api-ok/repos/cli/cli/releases/latest".to_string(),
            expected_headers: vec![
                (
                    "accept".to_string(),
                    "application/vnd.github+json".to_string(),
                ),
                ("user-agent".to_string(), "toolchain-installer".to_string()),
                ("x-github-api-version".to_string(), "2022-11-28".to_string()),
            ],
            status_line: "HTTP/1.1 200 OK",
            body,
        }];
        let (base, handle) = spawn_mock_server(responses);
        let client = reqwest::Client::new();

        let release = fetch_latest_release(
            &client,
            &[format!("{base}/api-ok")],
            "cli/cli",
            GitHubApiRequestOptions::new().with_user_agent("toolchain-installer"),
        )
        .await
        .expect("release");

        assert_eq!(release.tag_name, "v2.0.0");
        assert_eq!(release.assets[0].name, long_asset_name);
        handle.join().expect("mock server thread");
    }

    #[tokio::test]
    async fn fetch_latest_release_allows_local_api_base_without_bearer_token() {
        let responses = vec![MockHttpResponse {
            expected_path: "/api-ok/repos/cli/cli/releases/latest".to_string(),
            expected_headers: vec![
                (
                    "accept".to_string(),
                    "application/vnd.github+json".to_string(),
                ),
                ("user-agent".to_string(), "toolchain-installer".to_string()),
                ("x-github-api-version".to_string(), "2022-11-28".to_string()),
            ],
            status_line: "HTTP/1.1 200 OK",
            body: r#"{"tag_name":"v2.0.0","assets":[]}"#.to_string(),
        }];
        let (base, handle) = spawn_mock_server(responses);
        let client = reqwest::Client::new();

        let release = fetch_latest_release(
            &client,
            &[format!("{base}/api-ok")],
            "cli/cli",
            GitHubApiRequestOptions::new().with_user_agent("toolchain-installer"),
        )
        .await
        .expect("release");

        assert_eq!(release.tag_name, "v2.0.0");
        handle.join().expect("mock server thread");
    }

    #[tokio::test]
    async fn fetch_latest_release_rejects_insecure_api_base_when_bearer_token_present() {
        let client = reqwest::Client::new();

        let err = fetch_latest_release(
            &client,
            &["http://api.github.example.invalid/v3"],
            "cli/cli",
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .await
        .expect_err("insecure base should fail");

        let message = err.to_string();
        assert!(message.contains("https github api base"), "{message}");
    }

    #[tokio::test]
    async fn fetch_latest_release_rejects_local_api_base_when_bearer_token_present() {
        let client = reqwest::Client::new();

        let err = fetch_latest_release(
            &client,
            &["https://127.0.0.1/api"],
            "cli/cli",
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .await
        .expect_err("local base should fail");

        let message = err.to_string();
        assert!(message.contains("canonical GitHub API base"), "{message}");
        assert!(message.contains("127.0.0.1"), "{message}");
    }

    #[tokio::test]
    async fn fetch_latest_release_rejects_noncanonical_public_api_base_when_bearer_token_present() {
        let client = reqwest::Client::new();

        let err = fetch_latest_release(
            &client,
            &["https://github.example.com/api/v3"],
            "cli/cli",
            GitHubApiRequestOptions::new().with_bearer_token(Some("secret-token")),
        )
        .await
        .expect_err("noncanonical public base should fail");

        let message = err.to_string();
        assert!(message.contains("canonical GitHub API base"), "{message}");
    }

    #[tokio::test]
    async fn fetch_latest_release_rejects_custom_bearer_api_base_with_failed_dns() {
        let client = reqwest::Client::new();

        let err = fetch_latest_release(
            &client,
            &["https://github.example.invalid/api/v3"],
            "cli/cli",
            GitHubApiRequestOptions::new()
                .with_bearer_token(Some("secret-token"))
                .with_trusted_bearer_token_hosts(&["github.example.invalid"]),
        )
        .await
        .expect_err("failed dns should stop bearer token request");

        let message = err.to_string();
        assert!(message.contains("dns lookup"), "{message}");
    }

    #[tokio::test]
    async fn fetch_latest_release_redacts_sensitive_api_base_details_in_errors() {
        let client = reqwest::Client::new();

        let err = fetch_latest_release(
            &client,
            &["http://user:topsecret@127.0.0.1:9/api?token=top"],
            "cli/cli",
            GitHubApiRequestOptions::new(),
        )
        .await
        .expect_err("unreachable base should fail");

        let message = err.to_string();
        assert!(message.contains("127.0.0.1"), "{message}");
        assert!(!message.contains("user"), "{message}");
        assert!(!message.contains("topsecret"), "{message}");
        assert!(!message.contains("token=top"), "{message}");
        assert!(!message.contains("/api"), "{message}");
    }

    #[tokio::test]
    async fn fetch_latest_release_redacts_invalid_api_base_before_request() {
        let client = reqwest::Client::new();

        let err = fetch_latest_release(
            &client,
            &["http://user:topsecret@[::1]:99999/api?token=top"],
            "cli/cli",
            GitHubApiRequestOptions::new(),
        )
        .await
        .expect_err("invalid api base should fail");

        let message = err.to_string();
        assert!(message.contains("<redacted>"), "{message}");
        assert!(!message.contains("user"), "{message}");
        assert!(!message.contains("topsecret"), "{message}");
        assert!(!message.contains("token=top"), "{message}");
        assert!(!message.contains("/api"), "{message}");
    }

    struct MockHttpResponse {
        expected_path: String,
        expected_headers: Vec<(String, String)>,
        status_line: &'static str,
        body: String,
    }

    fn spawn_mock_server(responses: Vec<MockHttpResponse>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut request = Vec::new();
                let mut buf = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut buf).expect("read request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }

                let request_text = String::from_utf8(request).expect("utf8 request");
                let mut lines = request_text.lines();
                let first = lines.next().expect("request line");
                let path = first
                    .split_whitespace()
                    .nth(1)
                    .expect("request path")
                    .to_string();
                assert_eq!(path, response.expected_path);

                let lowercase = request_text.to_ascii_lowercase();
                for (name, value) in response.expected_headers {
                    let expected = format!("{}: {}", name.to_ascii_lowercase(), value);
                    assert!(
                        lowercase.contains(&expected.to_ascii_lowercase()),
                        "missing header `{expected}` in request:\n{request_text}"
                    );
                }

                let body = response.body;
                let reply = format!(
                    "{}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.status_line,
                    body.len(),
                    body
                );
                stream.write_all(reply.as_bytes()).expect("write reply");
                stream.flush().expect("flush reply");
            }
        });
        (format!("http://{addr}"), handle)
    }
}
