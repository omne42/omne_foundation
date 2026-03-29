use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitHubApiError {
    #[error("github repository must be `owner/repo`, got `{repo}`")]
    InvalidRepository { repo: String },
    #[error("invalid github api base: {details}")]
    InvalidApiBase { details: String },
    #[error("no usable github api base configured")]
    NoApiBaseConfigured,
    #[error("failed to fetch latest release metadata for {repo}: {details}")]
    LatestReleaseFetchFailed { repo: String, details: String },
}

pub type Result<T> = std::result::Result<T, GitHubApiError>;
