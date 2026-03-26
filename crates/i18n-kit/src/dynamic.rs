use std::fmt::{self, Display, Formatter};
use std::io;

use crate::Locale;

mod catalog;
mod composed;
mod locale_sources;
#[cfg(test)]
mod tests;

pub use self::catalog::{DynamicJsonCatalog, FallbackStrategy};
pub use self::composed::ComposedCatalog;
pub use self::locale_sources::{
    MAX_CATALOG_TOTAL_BYTES, MAX_LOCALE_SOURCE_BYTES, MAX_LOCALE_SOURCES,
    validate_locale_source_limits, validate_locale_source_path,
};

#[derive(Debug)]
pub enum DynamicCatalogError {
    Io(io::Error),
    Json(serde_json::Error),
    LocaleSourceJson {
        path: String,
        error: serde_json::Error,
    },
    InvalidLocaleIdentifier(String),
    InvalidLocaleFileName(String),
    DuplicateLocaleFile {
        locale: Locale,
        first_path: String,
        second_path: String,
    },
    TooManyLocaleSources {
        max: usize,
    },
    TooManyCatalogDirectories {
        max: usize,
    },
    CatalogDirectoryTooWide {
        path: String,
        entries: usize,
        max_entries: usize,
    },
    CatalogDirectoryTooDeep {
        path: String,
        depth: usize,
        max_depth: usize,
    },
    LocaleSourceTooLarge {
        path: String,
        bytes: usize,
        max_bytes: usize,
    },
    CatalogTooLarge {
        bytes: usize,
        max_bytes: usize,
    },
    MissingDefaultLocale(Locale),
}

impl Display for DynamicCatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::LocaleSourceJson { path, error } => {
                write!(f, "invalid locale source JSON in {path}: {error}")
            }
            Self::InvalidLocaleIdentifier(locale) => {
                write!(f, "invalid locale identifier: {locale}")
            }
            Self::InvalidLocaleFileName(path) => {
                write!(f, "invalid locale file name: {path}")
            }
            Self::DuplicateLocaleFile {
                locale,
                first_path,
                second_path,
            } => write!(
                f,
                "duplicate locale file for {locale}: {first_path} and {second_path}"
            ),
            Self::TooManyLocaleSources { max } => {
                write!(f, "catalog exceeds locale source limit of {max} files")
            }
            Self::TooManyCatalogDirectories { max } => {
                write!(
                    f,
                    "catalog exceeds directory traversal limit of {max} directories"
                )
            }
            Self::CatalogDirectoryTooWide {
                path,
                entries,
                max_entries,
            } => write!(
                f,
                "catalog directory exceeds entry limit ({entries} > {max_entries}): {path}"
            ),
            Self::CatalogDirectoryTooDeep {
                path,
                depth,
                max_depth,
            } => write!(
                f,
                "catalog directory exceeds depth limit ({depth} > {max_depth}): {path}"
            ),
            Self::LocaleSourceTooLarge {
                path,
                bytes,
                max_bytes,
            } => write!(
                f,
                "catalog locale source exceeds size limit ({bytes} > {max_bytes} bytes): {path}"
            ),
            Self::CatalogTooLarge { bytes, max_bytes } => write!(
                f,
                "catalog exceeds total size limit ({bytes} > {max_bytes} bytes)"
            ),
            Self::MissingDefaultLocale(locale) => {
                write!(f, "default locale is missing from catalog: {locale}")
            }
        }
    }
}

impl std::error::Error for DynamicCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::LocaleSourceJson { error, .. } => Some(error),
            Self::InvalidLocaleIdentifier(_)
            | Self::InvalidLocaleFileName(_)
            | Self::DuplicateLocaleFile { .. }
            | Self::TooManyLocaleSources { .. }
            | Self::TooManyCatalogDirectories { .. }
            | Self::CatalogDirectoryTooWide { .. }
            | Self::CatalogDirectoryTooDeep { .. }
            | Self::LocaleSourceTooLarge { .. }
            | Self::CatalogTooLarge { .. }
            | Self::MissingDefaultLocale(_) => None,
        }
    }
}

impl From<io::Error> for DynamicCatalogError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DynamicCatalogError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
