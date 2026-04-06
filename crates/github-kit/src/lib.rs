#![forbid(unsafe_code)]

mod client;
mod error;
mod release;

pub use client::{
    DEFAULT_GITHUB_API_BASE, DEFAULT_GITHUB_API_VERSION, DEFAULT_GITHUB_USER_AGENT,
    GITHUB_API_ACCEPT, GitHubApiRequestOptions, apply_github_api_headers, build_github_api_url,
    validate_github_api_request_url, validate_github_api_request_url_dns,
};
pub use error::{GitHubApiError, Result};
pub use release::{
    GitHubRelease, GitHubReleaseAsset, fetch_latest_release, fetch_latest_release_with_profile,
};
