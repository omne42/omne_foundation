use std::path::PathBuf;

use policy_meta::ArtifactError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("missing path after {flag}")]
    MissingPath { flag: &'static str },
    #[error("unknown argument: {arg}")]
    UnknownArgument { arg: String },
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
}

pub fn next_path_arg(flag: &'static str, value: Option<String>) -> Result<PathBuf, CliError> {
    value
        .map(PathBuf::from)
        .ok_or(CliError::MissingPath { flag })
}
