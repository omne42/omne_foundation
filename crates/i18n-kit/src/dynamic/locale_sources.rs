use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use omne_systems_fs_primitives::{DEFAULT_TEXT_FILE_BYTES_LIMIT, DEFAULT_TEXT_TREE_BYTES_LIMIT};

use crate::Locale;

use super::{
    DynamicCatalogError, directory_loader::read_locale_sources_from_directory,
    parse_json_catalog_sources, parse_json_catalog_text_map,
};

pub(super) const MAX_LOCALE_SOURCES: usize = 256;
pub(super) const MAX_LOCALE_SOURCE_BYTES: usize = DEFAULT_TEXT_FILE_BYTES_LIMIT;
pub(super) const MAX_CATALOG_TOTAL_BYTES: usize = DEFAULT_TEXT_TREE_BYTES_LIMIT;
pub(super) const MAX_CATALOG_DIRECTORIES: usize = 2048;
pub(super) const MAX_CATALOG_DIRECTORY_DEPTH: usize = 32;

pub(super) type LocaleMap = BTreeMap<Locale, BTreeMap<String, Arc<str>>>;

pub(super) fn load_locales_from_directory(
    path: &Path,
    default_locale: Locale,
) -> Result<LocaleMap, DynamicCatalogError> {
    let sources = read_locale_sources_from_directory(path)?;
    load_locales_from_sources(sources, default_locale)
}

pub(super) fn load_locales_from_json(
    json: &str,
    default_locale: Locale,
) -> Result<LocaleMap, DynamicCatalogError> {
    validate_catalog_json_input(json)?;
    let data = parse_json_catalog_sources(json)?;
    if data.len() > MAX_LOCALE_SOURCES {
        return Err(DynamicCatalogError::TooManyLocaleSources {
            max: MAX_LOCALE_SOURCES,
        });
    }

    let mut locales = LocaleMap::new();
    let mut source_count = 0usize;
    for (locale_str, texts_json) in data {
        source_count += 1;
        validate_locale_source_limits(
            Path::new(&locale_str),
            source_count,
            texts_json.get().len(),
            json.len(),
        )?;

        let locale = Locale::parse_canonical(&locale_str)
            .ok_or_else(|| DynamicCatalogError::InvalidLocaleIdentifier(locale_str.clone()))?;
        let texts = parse_json_catalog_text_map(texts_json.get()).map_err(|error| {
            DynamicCatalogError::LocaleSourceJson {
                path: locale_str.clone(),
                error,
            }
        })?;
        let previous = locales.insert(locale, texts);
        debug_assert!(
            previous.is_none(),
            "UniqueLocaleMap must reject duplicate canonical locale keys"
        );
    }

    ensure_default_locale_present(&locales, default_locale)?;
    Ok(locales)
}

pub(super) fn load_locales_from_sources<I, P>(
    sources: I,
    default_locale: Locale,
) -> Result<LocaleMap, DynamicCatalogError>
where
    I: IntoIterator<Item = (P, String)>,
    P: Into<PathBuf>,
{
    let mut locales = LocaleMap::new();
    let mut locale_sources = BTreeMap::<Locale, PathBuf>::new();
    let mut source_count = 0usize;
    let mut total_bytes = 0usize;
    for (source_path, content) in sources {
        let source_path = source_path.into();
        source_count += 1;
        total_bytes = total_bytes.saturating_add(content.len());
        validate_locale_source_limits(&source_path, source_count, content.len(), total_bytes)?;

        let file_name = locale_id_from_source_path(&source_path)?;
        let locale = Locale::parse_canonical(file_name).ok_or_else(|| {
            DynamicCatalogError::InvalidLocaleFileName(source_path.display().to_string())
        })?;

        if let Some(existing) = locale_sources.insert(locale, source_path.clone()) {
            return Err(DynamicCatalogError::DuplicateLocaleFile {
                locale,
                first_path: existing.display().to_string(),
                second_path: source_path.display().to_string(),
            });
        }

        let texts = parse_json_catalog_text_map(&content).map_err(|error| {
            DynamicCatalogError::LocaleSourceJson {
                path: source_path.display().to_string(),
                error,
            }
        })?;
        locales.insert(locale, texts);
    }

    ensure_default_locale_present(&locales, default_locale)?;
    Ok(locales)
}

fn locale_id_from_source_path(source_path: &Path) -> Result<&str, DynamicCatalogError> {
    let invalid_name =
        || DynamicCatalogError::InvalidLocaleFileName(source_path.display().to_string());

    match source_path.extension().and_then(|value| value.to_str()) {
        Some("json") => source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(invalid_name),
        Some(_) => Err(invalid_name()),
        None => source_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(invalid_name),
    }
}

pub(super) fn validate_locale_source_limits(
    path: &Path,
    source_count: usize,
    source_bytes: usize,
    total_bytes: usize,
) -> Result<(), DynamicCatalogError> {
    if source_count > MAX_LOCALE_SOURCES {
        return Err(DynamicCatalogError::TooManyLocaleSources {
            max: MAX_LOCALE_SOURCES,
        });
    }
    if source_bytes > MAX_LOCALE_SOURCE_BYTES {
        return Err(DynamicCatalogError::LocaleSourceTooLarge {
            path: path.display().to_string(),
            bytes: source_bytes,
            max_bytes: MAX_LOCALE_SOURCE_BYTES,
        });
    }
    if total_bytes > MAX_CATALOG_TOTAL_BYTES {
        return Err(DynamicCatalogError::CatalogTooLarge {
            bytes: total_bytes,
            max_bytes: MAX_CATALOG_TOTAL_BYTES,
        });
    }

    Ok(())
}

fn validate_catalog_json_input(json: &str) -> Result<(), DynamicCatalogError> {
    if json.len() > MAX_CATALOG_TOTAL_BYTES {
        return Err(DynamicCatalogError::CatalogTooLarge {
            bytes: json.len(),
            max_bytes: MAX_CATALOG_TOTAL_BYTES,
        });
    }

    Ok(())
}

pub(super) fn validate_catalog_directory_count(count: usize) -> Result<(), DynamicCatalogError> {
    if count > MAX_CATALOG_DIRECTORIES {
        return Err(DynamicCatalogError::TooManyCatalogDirectories {
            max: MAX_CATALOG_DIRECTORIES,
        });
    }

    Ok(())
}

pub(super) fn validate_catalog_directory_depth(
    path: &Path,
    depth: usize,
) -> Result<(), DynamicCatalogError> {
    if depth > MAX_CATALOG_DIRECTORY_DEPTH {
        return Err(DynamicCatalogError::CatalogDirectoryTooDeep {
            path: path.display().to_string(),
            depth,
            max_depth: MAX_CATALOG_DIRECTORY_DEPTH,
        });
    }

    Ok(())
}

fn ensure_default_locale_present(
    locales: &LocaleMap,
    default_locale: Locale,
) -> Result<(), DynamicCatalogError> {
    if locales.contains_key(&default_locale) {
        return Ok(());
    }
    Err(DynamicCatalogError::MissingDefaultLocale(default_locale))
}
