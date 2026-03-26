use std::io;
use std::path::PathBuf;

use crate::ConfigFormat;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid config load options: {message}")]
    InvalidOptions { message: String },

    #[error("unsupported config format for {path}: {message}")]
    UnsupportedFormat { path: PathBuf, message: String },

    #[error("config path contains a symlink or reparse point: {path}")]
    SymlinkPath { path: PathBuf },

    #[error("config path escapes root: {path} (root={root})")]
    PathEscapesRoot { root: PathBuf, path: PathBuf },

    #[error("config file is too large: {size_bytes} bytes (max {max_bytes}): {path}")]
    FileTooLarge {
        path: PathBuf,
        size_bytes: u64,
        max_bytes: u64,
    },

    #[error("config file is not valid UTF-8: {path}: {message}")]
    InvalidUtf8 { path: PathBuf, message: String },

    #[error("failed to {action} config file {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse {format} config{location}: {message}")]
    Parse {
        format: ConfigFormat,
        location: String,
        message: String,
    },

    #[error("config format {format} is not allowed{location}: expected {expected}")]
    FormatNotAllowed {
        format: ConfigFormat,
        location: String,
        expected: String,
    },

    #[error("required config layer {name} not found{location}")]
    RequiredLayerMissing { name: String, location: String },

    #[error("failed to serialize {format} config: {message}")]
    Serialize {
        format: ConfigFormat,
        message: String,
    },

    #[error("config env interpolation: {message}")]
    EnvInterpolation { message: String },
}

pub type Result<T> = std::result::Result<T, Error>;
