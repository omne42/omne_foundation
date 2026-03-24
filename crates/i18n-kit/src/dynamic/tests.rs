use super::locale_sources::{
    MAX_CATALOG_DIRECTORIES, MAX_CATALOG_DIRECTORY_DEPTH, MAX_CATALOG_TOTAL_BYTES,
    MAX_LOCALE_SOURCE_BYTES, MAX_LOCALE_SOURCES,
};
use super::*;
use crate::{Catalog, TranslationCatalog, TranslationResolution};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn generated_locale(index: usize) -> String {
    let first = ((index / (26 * 26)) % 26) as u8 + b'a';
    let second = ((index / 26) % 26) as u8 + b'a';
    let third = (index % 26) as u8 + b'a';
    String::from_utf8(vec![first, second, third]).expect("generated locale should be ASCII")
}

#[test]
fn dynamic_catalog_from_json_string() {
    let json = r#"
    {
        "en_US": { "greeting": "hello {name}" },
        "fr_FR": { "greeting": "bonjour {name}" }
    }
    "#;

    let catalog = DynamicJsonCatalog::from_json_string(json, Locale::EN_US, FallbackStrategy::Both)
        .expect("load catalog");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "greeting"),
        Some("hello {name}".to_string())
    );
    assert_eq!(
        catalog.get_text(Locale::parse("fr_FR").expect("fr"), "greeting"),
        Some("bonjour {name}".to_string())
    );
}

#[test]
fn dynamic_catalog_fallback_strategy_return_default_locale() {
    let json = r#"
    {
        "en_US": { "greeting": "hello" }
    }
    "#;

    let catalog = DynamicJsonCatalog::from_json_string(
        json,
        Locale::EN_US,
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("load catalog");

    assert_eq!(
        catalog.get_text(Locale::parse("fr_FR").expect("fr"), "greeting"),
        Some("hello".to_string())
    );
    assert_eq!(
        catalog.get_text(Locale::parse("fr_FR").expect("fr"), "missing"),
        None
    );
    assert_eq!(catalog.get_text(Locale::EN_US, "missing"), None);
}

#[test]
fn composed_catalog_tries_multiple_sources() {
    let json1 = r#"{ "en_US": { "greeting": "hello" } }"#;
    let json2 = r#"{ "en_US": { "farewell": "goodbye" }, "fr_FR": { "greeting": "bonjour" } }"#;

    let catalog1 =
        DynamicJsonCatalog::from_json_string(json1, Locale::EN_US, FallbackStrategy::ReturnKey)
            .expect("catalog1");
    let catalog2 =
        DynamicJsonCatalog::from_json_string(json2, Locale::EN_US, FallbackStrategy::ReturnKey)
            .expect("catalog2");

    let composed = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(catalog1)
        .add_catalog(catalog2);

    assert_eq!(
        composed.get_text(Locale::EN_US, "greeting"),
        Some("hello".to_string())
    );
    assert_eq!(
        composed.get_text(Locale::EN_US, "farewell"),
        Some("goodbye".to_string())
    );
    assert_eq!(
        composed.available_locales(),
        vec![Locale::EN_US, Locale::parse("fr_FR").expect("fr_FR")]
    );
}

#[test]
fn composed_catalog_prefers_exact_match_over_earlier_catalog_fallback() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect("early");
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "unused": "unused" }, "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("late");

    let composed = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(early)
        .add_catalog(late);

    assert_eq!(
        composed.get_text(Locale::parse("fr_FR").expect("fr_FR"), "greeting"),
        Some("bonjour".to_string())
    );
}

#[test]
fn composed_catalog_marks_later_exact_match_as_exact_resolution() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect("early");
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "unused": "unused" }, "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("late");

    let composed = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(early)
        .add_catalog(late);

    assert!(matches!(
        composed.resolve_shared(Locale::parse("fr_FR").expect("fr_FR"), "greeting"),
        TranslationResolution::Exact(value) if value.as_ref() == "bonjour"
    ));
}

#[test]
fn nested_composed_catalog_preserves_exact_lookup() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect("early");
    let nested = ComposedCatalog::new(Locale::EN_US).add_catalog(early);
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "unused": "unused" }, "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("late");

    let outer = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(nested)
        .add_catalog(late);

    assert_eq!(
        outer.get_text(Locale::parse("fr_FR").expect("fr_FR"), "greeting"),
        Some("bonjour".to_string())
    );
}

#[test]
fn composed_catalog_preserves_identity_translation_hits() {
    let exact = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "unused": "unused" }, "fr_FR": { "status": "status" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("exact");
    let fallback = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "status": "english" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("fallback");

    let composed = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(exact)
        .add_catalog(fallback);

    assert_eq!(
        composed.get_text(Locale::parse("fr_FR").expect("fr_FR"), "status"),
        Some("status".to_string())
    );
}

#[test]
fn composed_catalog_preserves_identity_translation_from_catalog_fallback() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "status": "status" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("early");
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "status": "english" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("late");

    let composed = ComposedCatalog::new(Locale::parse("fr_FR").expect("fr_FR"))
        .add_catalog(early)
        .add_catalog(late);

    let locale = Locale::parse("de_DE").expect("de_DE");
    assert_eq!(
        composed.get_text(locale, "status"),
        Some("status".to_string())
    );
    assert_eq!(
        composed
            .get_template_shared(locale, "status")
            .map(|value| value.to_string()),
        Some("status".to_string())
    );
}

#[test]
fn composed_catalog_preserves_late_catalog_default_fallback_after_exact_miss() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "fr_FR": { "farewell": "au revoir" } }"#,
        Locale::parse("fr_FR").expect("fr_FR"),
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("early");
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("late");

    let composed = ComposedCatalog::new(Locale::parse("fr_FR").expect("fr_FR"))
        .add_catalog(early)
        .add_catalog(late);

    assert_eq!(
        composed.get_text(Locale::parse("de_DE").expect("de_DE"), "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn composed_catalog_ignores_early_return_key_when_later_catalog_can_fallback() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "fr_FR": { "farewell": "au revoir" } }"#,
        Locale::parse("fr_FR").expect("fr_FR"),
        FallbackStrategy::ReturnKey,
    )
    .expect("early");
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("late");

    let composed = ComposedCatalog::new(Locale::parse("fr_FR").expect("fr_FR"))
        .add_catalog(early)
        .add_catalog(late);

    assert_eq!(
        composed.get_text(Locale::parse("de_DE").expect("de_DE"), "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn composed_catalog_prefers_earlier_catalog_fallback_before_composed_default_locale() {
    let early = DynamicJsonCatalog::from_json_string(
        r#"{ "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::parse("fr_FR").expect("fr_FR"),
        FallbackStrategy::ReturnDefaultLocale,
    )
    .expect("early");
    let late = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("late");

    let composed = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(early)
        .add_catalog(late);

    let locale = Locale::parse("de_DE").expect("de_DE");
    assert_eq!(
        composed.get_text(locale, "greeting"),
        Some("bonjour".to_string())
    );
    assert_eq!(
        composed
            .get_template_shared(locale, "greeting")
            .map(|value| value.to_string()),
        Some("bonjour".to_string())
    );
}

#[test]
fn composed_catalog_only_reports_real_available_locales() {
    let catalog = DynamicJsonCatalog::from_json_string(
        r#"{ "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::parse("fr_FR").expect("fr_FR"),
        FallbackStrategy::ReturnKey,
    )
    .expect("catalog");

    let composed = ComposedCatalog::new(Locale::EN_US).add_catalog(catalog);

    assert_eq!(
        composed.available_locales(),
        vec![Locale::parse("fr_FR").expect("fr_FR")]
    );
    assert!(!composed.locale_enabled(Locale::EN_US));
}

#[test]
fn composed_catalog_does_not_guess_default_locale_when_default_is_absent() {
    let catalog = DynamicJsonCatalog::from_json_string(
        r#"{ "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::parse("fr_FR").expect("fr_FR"),
        FallbackStrategy::ReturnKey,
    )
    .expect("catalog");

    let composed = ComposedCatalog::new(Locale::EN_US).add_catalog(catalog);

    assert_eq!(Catalog::default_locale(&composed), Locale::EN_US);
    assert_eq!(
        composed.try_resolve_locale(None),
        Err(crate::ResolveLocaleError::LocaleNotEnabled {
            requested: "en_US".to_string(),
            available: vec![Locale::parse("fr_FR").expect("fr_FR")],
        })
    );
    assert_eq!(
        composed.get_text(Locale::parse("de_DE").expect("de_DE"), "greeting"),
        Some("greeting".to_string())
    );
}

#[test]
fn composed_catalog_keeps_full_miss_as_missing_without_synthetic_fallback() {
    let catalog = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(
            DynamicJsonCatalog::from_json_string(
                r#"{ "en_US": { "hello": "hello" } }"#,
                Locale::EN_US,
                FallbackStrategy::ReturnDefaultLocale,
            )
            .expect("catalog"),
        )
        .add_catalog(
            DynamicJsonCatalog::from_json_string(
                r#"{ "fr_FR": { "bonjour": "bonjour" } }"#,
                Locale::parse("fr_FR").expect("fr_FR"),
                FallbackStrategy::ReturnDefaultLocale,
            )
            .expect("catalog"),
        );

    let locale = Locale::parse("de_DE").expect("de_DE");
    assert!(matches!(
        catalog.resolve_shared(locale, "missing"),
        TranslationResolution::Missing
    ));
    assert_eq!(catalog.get_text(locale, "missing"), None);
    assert_eq!(catalog.get_template_shared(locale, "missing"), None);
}

#[test]
fn dynamic_catalog_loads_nested_locale_files() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("nested")).expect("mkdir");
    fs::write(
        temp.path().join("nested").join("en_US.json"),
        r#"{"greeting":"hello"}"#,
    )
    .expect("write nested locale");

    let catalog =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect("load nested catalog");
    assert_eq!(
        catalog.get_text(Locale::EN_US, "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn dynamic_catalog_loads_many_sibling_directories() {
    let temp = TempDir::new().expect("temp dir");
    for index in 0..2048 {
        fs::create_dir_all(temp.path().join(format!("dir_{index:04}"))).expect("mkdir sibling");
    }
    fs::write(
        temp.path().join("dir_2047").join("en_US.json"),
        r#"{"greeting":"hello"}"#,
    )
    .expect("write locale");

    let catalog =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect("load wide catalog");
    assert_eq!(
        catalog.get_text(Locale::EN_US, "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn dynamic_catalog_rejects_excessive_directory_depth() {
    let temp = TempDir::new().expect("temp dir");
    let mut deepest = temp.path().to_path_buf();
    for index in 0..=MAX_CATALOG_DIRECTORY_DEPTH {
        deepest = deepest.join(format!("nested_{index:02}"));
        fs::create_dir_all(&deepest).expect("mkdir nested");
    }
    fs::write(deepest.join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("overly deep catalogs should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::CatalogDirectoryTooDeep {
            depth,
            max_depth,
            ..
        } if depth == MAX_CATALOG_DIRECTORY_DEPTH + 1
            && max_depth == MAX_CATALOG_DIRECTORY_DEPTH
    ));
}

#[test]
fn dynamic_catalog_rejects_excessive_directory_count() {
    let temp = TempDir::new().expect("temp dir");
    for index in 0..=MAX_CATALOG_DIRECTORIES {
        let dir = temp.path().join(format!("dir_{index:04}"));
        fs::create_dir_all(&dir).expect("mkdir sibling");
        if index == 0 {
            fs::write(dir.join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");
        }
    }

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("catalogs with too many directories should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::TooManyCatalogDirectories { max }
            if max == MAX_CATALOG_DIRECTORIES
    ));
}

#[test]
fn dynamic_catalog_errors_when_default_locale_is_missing() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("zh_CN.json"), r#"{"greeting":"nihao"}"#).expect("write locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("missing default locale should fail");
    let DynamicCatalogError::MissingDefaultLocale(locale) = err else {
        panic!("expected missing default locale error");
    };
    assert_eq!(locale, Locale::EN_US);
}

#[test]
fn dynamic_catalog_from_json_string_requires_default_locale() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "fr_FR": { "greeting": "bonjour" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("missing default locale should fail");
    let DynamicCatalogError::MissingDefaultLocale(locale) = err else {
        panic!("expected missing default locale error");
    };
    assert_eq!(locale, Locale::EN_US);
}

#[test]
fn dynamic_catalog_from_json_string_rejects_invalid_locale_keys() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "definitely-not-a-locale": { "greeting": "bonjour" }, "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("invalid locale key should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::InvalidLocaleIdentifier(locale)
            if locale == "definitely-not-a-locale"
    ));
}

#[test]
fn dynamic_catalog_from_json_string_accepts_distinct_language_and_region_keys() {
    let catalog = DynamicJsonCatalog::from_json_string(
        r#"{ "en": { "greeting": "generic" }, "en_US": { "greeting": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect("language and region keys should coexist");

    assert_eq!(
        catalog
            .lookup_shared(Locale::parse_canonical("en").expect("en"), "greeting")
            .as_deref(),
        Some("generic")
    );
    assert_eq!(
        catalog.lookup_shared(Locale::EN_US, "greeting").as_deref(),
        Some("hello")
    );
}

#[test]
fn dynamic_catalog_from_json_string_rejects_noncanonical_locale_keys() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US.UTF-8": { "greeting": "hello" }, "en_US": { "fallback": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("noncanonical locale key should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::InvalidLocaleIdentifier(locale)
            if locale == "en_US.UTF-8"
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_duplicate_raw_locale_keys() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello" }, "en_US": { "greeting": "hi" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("duplicate raw locale keys should fail");

    assert!(matches!(err, DynamicCatalogError::Json(_)));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_duplicate_catalog_keys() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello", "greeting": "hi" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("duplicate catalog keys should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceJson { path, error }
            if path == "en_US"
                && error.to_string().contains("duplicate catalog key: greeting")
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_invalid_catalog_keys() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "bad key": "hello" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("invalid catalog key should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceJson { path, error }
            if path == "en_US"
                && error.to_string().contains("invalid catalog key: bad key")
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_unclosed_catalog_placeholders() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello {name" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("invalid catalog template should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceJson { path, error }
            if path == "en_US"
                && error
                .to_string()
                .contains("invalid catalog template for greeting: unclosed placeholder")
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_invalid_placeholder_names() {
    let err = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "greeting": "hello {first name}" } }"#,
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("invalid placeholder name should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceJson { path, error }
            if path == "en_US"
                && error.to_string().contains(
                "invalid catalog template for greeting: invalid placeholder name: first name"
            )
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_excessive_total_bytes() {
    let payload = "x".repeat(MAX_CATALOG_TOTAL_BYTES);
    let json = format!(r#"{{ "en_US": {{ "greeting": "{payload}" }} }}"#);

    let err = DynamicJsonCatalog::from_json_string(&json, Locale::EN_US, FallbackStrategy::Both)
        .expect_err("oversized JSON catalog should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::CatalogTooLarge {
            max_bytes,
            ..
        } if max_bytes == MAX_CATALOG_TOTAL_BYTES
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_excessive_locale_count() {
    let mut entries = Vec::with_capacity(MAX_LOCALE_SOURCES + 1);
    for index in 0..=MAX_LOCALE_SOURCES {
        let locale = generated_locale(index);
        entries.push(format!(r#""{locale}": {{"greeting":"hi"}}"#));
    }
    let json = format!("{{ {} }}", entries.join(", "));
    let default_locale =
        Locale::parse_canonical(&generated_locale(0)).expect("generated default locale");

    let err = DynamicJsonCatalog::from_json_string(&json, default_locale, FallbackStrategy::Both)
        .expect_err("too many locale entries should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::TooManyLocaleSources { max } if max == MAX_LOCALE_SOURCES
    ));
}

#[test]
fn dynamic_catalog_errors_on_duplicate_locale_files() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("nested")).expect("mkdir");
    fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hello"}"#)
        .expect("write root locale");
    fs::write(
        temp.path().join("nested").join("en_US.json"),
        r#"{"greeting":"hi"}"#,
    )
    .expect("write nested locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("duplicate locale should fail");
    assert!(matches!(
        err,
        DynamicCatalogError::DuplicateLocaleFile { .. }
    ));
}

#[test]
fn dynamic_catalog_reports_duplicate_locale_files_in_stable_path_order() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("a")).expect("mkdir a");
    fs::create_dir_all(temp.path().join("b")).expect("mkdir b");
    fs::write(
        temp.path().join("a").join("en_US.json"),
        r#"{"greeting":"hello"}"#,
    )
    .expect("write a locale");
    fs::write(
        temp.path().join("b").join("en_US.json"),
        r#"{"greeting":"hi"}"#,
    )
    .expect("write b locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("duplicate locale should fail");

    let DynamicCatalogError::DuplicateLocaleFile {
        first_path,
        second_path,
        ..
    } = err
    else {
        panic!("expected duplicate locale file error");
    };

    assert!(
        Path::new(&first_path).ends_with(Path::new("a").join("en_US.json")),
        "{first_path}"
    );
    assert!(
        Path::new(&second_path).ends_with(Path::new("b").join("en_US.json")),
        "{second_path}"
    );
}

#[test]
fn dynamic_catalog_reload_keeps_previous_snapshot_visible_until_swap() {
    struct BlockingSources {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        source: (PathBuf, String),
    }

    struct BlockingSourcesIter {
        entered: Option<mpsc::Sender<()>>,
        release: Option<mpsc::Receiver<()>>,
        source: Option<(PathBuf, String)>,
    }

    impl IntoIterator for BlockingSources {
        type Item = (PathBuf, String);
        type IntoIter = BlockingSourcesIter;

        fn into_iter(self) -> Self::IntoIter {
            BlockingSourcesIter {
                entered: Some(self.entered),
                release: Some(self.release),
                source: Some(self.source),
            }
        }
    }

    impl Iterator for BlockingSourcesIter {
        type Item = (PathBuf, String);

        fn next(&mut self) -> Option<Self::Item> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).expect("signal reload started");
                self.release
                    .take()
                    .expect("release receiver")
                    .recv()
                    .expect("release reload");
            }

            self.source.take()
        }
    }

    let catalog = Arc::new(
        DynamicJsonCatalog::from_json_string(
            r#"{ "en_US": { "greeting": "hello" } }"#,
            Locale::EN_US,
            FallbackStrategy::Both,
        )
        .expect("load initial catalog"),
    );
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let reloading_catalog = Arc::clone(&catalog);
    let reload = thread::spawn(move || {
        reloading_catalog
            .reload_from_locale_sources(BlockingSources {
                entered: entered_tx,
                release: release_rx,
                source: (
                    PathBuf::from("en_US.json"),
                    r#"{ "greeting": "hi" }"#.to_string(),
                ),
            })
            .expect("reload catalog");
    });

    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("reload should begin preparing a new snapshot");

    let reader_catalog = Arc::clone(&catalog);
    let (read_tx, read_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        read_tx
            .send(reader_catalog.get_text(Locale::EN_US, "greeting"))
            .expect("send old snapshot");
    });

    let observed = read_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("reads should stay on the old snapshot while reload is preparing");
    assert_eq!(observed, Some("hello".to_string()));

    release_tx.send(()).expect("release reload");
    reader.join().expect("join reader");
    reload.join().expect("join reload");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "greeting"),
        Some("hi".to_string())
    );
}

#[test]
fn dynamic_catalog_errors_on_invalid_locale_file_name() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hello"}"#)
        .expect("write default locale");
    fs::write(
        temp.path().join("definitely-not-a-locale.json"),
        r#"{"greeting":"bad"}"#,
    )
    .expect("write invalid locale file");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("invalid locale file should fail");

    assert!(matches!(err, DynamicCatalogError::InvalidLocaleFileName(_)));
}

#[test]
fn dynamic_catalog_from_directory_accepts_extensionless_names() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("en_US"), r#"{"greeting":"hello"}"#).expect("write locale");

    let catalog =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect("extensionless locale file should load");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn dynamic_catalog_from_directory_rejects_non_json_extensions() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("en_US.txt"), r#"{"greeting":"hello"}"#).expect("write locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("unexpected extension should be rejected");

    assert!(matches!(
        err,
        DynamicCatalogError::InvalidLocaleFileName(path) if path.ends_with("en_US.txt")
    ));
}

#[test]
fn dynamic_catalog_from_locale_sources_accepts_extensionless_names() {
    let catalog = DynamicJsonCatalog::from_locale_sources(
        vec![("en_US", r#"{"greeting":"hello"}"#.to_string())],
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect("extensionless locale source should load");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn dynamic_catalog_from_locale_sources_rejects_non_json_extensions() {
    let err = DynamicJsonCatalog::from_locale_sources(
        vec![("en_US.txt", r#"{"greeting":"hello"}"#.to_string())],
        Locale::EN_US,
        FallbackStrategy::Both,
    )
    .expect_err("unexpected extension should be rejected");

    assert!(matches!(
        err,
        DynamicCatalogError::InvalidLocaleFileName(path) if path == "en_US.txt"
    ));
}

#[cfg(unix)]
#[test]
fn dynamic_catalog_rejects_symlinked_locale_file() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let outside_dir = TempDir::new().expect("outside dir");
    let outside = outside_dir.path().join("en_US.json");
    fs::write(&outside, r#"{"greeting":"hello"}"#).expect("write outside locale");
    symlink(&outside, temp.path().join("en_US.json")).expect("create symlink");

    let error =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("symlinked locale file should be rejected");
    let DynamicCatalogError::Io(error) = error else {
        panic!("expected io error");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[cfg(unix)]
#[test]
fn dynamic_catalog_rejects_non_utf8_path_components() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = TempDir::new().expect("temp dir");
    let invalid = OsString::from_vec(vec![0x66, 0x6f, 0x80]);
    let nested = temp.path().join(&invalid);
    fs::create_dir_all(&nested).expect("mkdir invalid path");
    fs::write(nested.join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");

    let error =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("non-utf8 path should fail");
    let DynamicCatalogError::Io(error) = error else {
        panic!("expected io error");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("not valid UTF-8"));
}

#[cfg(unix)]
#[test]
fn dynamic_catalog_rejects_socket_entries() {
    use std::os::unix::net::UnixListener;

    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");
    let socket_path = temp.path().join("catalog.sock");
    let _listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping dynamic_catalog_rejects_socket_entries: unix socket bind not permitted in this environment: {err}"
            );
            return;
        }
        Err(err) => panic!("bind socket: {err}"),
    };

    let error =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("socket entries should fail");
    let DynamicCatalogError::Io(error) = error else {
        panic!("expected io error");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("regular file or directory"));
}

#[cfg(unix)]
#[test]
fn dynamic_catalog_rejects_symlinked_root_path() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let outside = TempDir::new().expect("outside dir");
    fs::write(outside.path().join("en_US.json"), r#"{"greeting":"hello"}"#).expect("write locale");
    let root = temp.path().join("linked_root");
    symlink(outside.path(), &root).expect("create root symlink");

    let error = DynamicJsonCatalog::from_directory(&root, Locale::EN_US, FallbackStrategy::Both)
        .expect_err("symlinked root should fail");
    let DynamicCatalogError::Io(error) = error else {
        panic!("expected io error");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[cfg(unix)]
#[test]
fn dynamic_catalog_rejects_root_path_with_symlinked_ancestor() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let outside = TempDir::new().expect("outside dir");
    fs::create_dir_all(outside.path().join("nested")).expect("mkdir nested");
    fs::write(
        outside.path().join("nested").join("en_US.json"),
        r#"{"greeting":"hello"}"#,
    )
    .expect("write locale");
    symlink(outside.path(), temp.path().join("linked")).expect("create ancestor symlink");
    let root = temp.path().join("linked").join("nested");

    let error = DynamicJsonCatalog::from_directory(&root, Locale::EN_US, FallbackStrategy::Both)
        .expect_err("symlinked ancestor should fail");
    let DynamicCatalogError::Io(error) = error else {
        panic!("expected io error");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn dynamic_catalog_errors_when_root_directory_is_missing() {
    let temp = TempDir::new().expect("temp dir");
    let missing = temp.path().join("missing");

    let error = DynamicJsonCatalog::from_directory(&missing, Locale::EN_US, FallbackStrategy::Both)
        .expect_err("missing root should fail");
    let DynamicCatalogError::Io(error) = error else {
        panic!("expected io error");
    };
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn dynamic_catalog_errors_on_duplicate_catalog_keys_in_file() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(
        temp.path().join("en_US.json"),
        r#"{"greeting":"hello","greeting":"hi"}"#,
    )
    .expect("write locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("duplicate catalog keys should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceJson { path, error }
            if path.ends_with("en_US.json")
                && error.to_string().contains("duplicate catalog key: greeting")
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_oversized_locale_source() {
    let oversized = "x".repeat(MAX_LOCALE_SOURCE_BYTES);
    let json = format!(r#"{{ "en_US": {{ "greeting": "{oversized}" }} }}"#);

    let err = DynamicJsonCatalog::from_json_string(&json, Locale::EN_US, FallbackStrategy::Both)
        .expect_err("oversized inline locale source should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceTooLarge {
            max_bytes,
            ..
        } if max_bytes == MAX_LOCALE_SOURCE_BYTES
    ));
}

#[test]
fn dynamic_catalog_from_json_string_rejects_oversized_raw_locale_source() {
    let padding = " ".repeat(MAX_LOCALE_SOURCE_BYTES);
    let json = format!(r#"{{"en_US":{{{padding}"greeting":"hi"}}}}"#);

    let err = DynamicJsonCatalog::from_json_string(&json, Locale::EN_US, FallbackStrategy::Both)
        .expect_err("oversized raw locale source should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceTooLarge {
            max_bytes,
            ..
        } if max_bytes == MAX_LOCALE_SOURCE_BYTES
    ));
}

#[test]
fn dynamic_catalog_rejects_oversized_locale_source() {
    let temp = TempDir::new().expect("temp dir");
    let oversized = "x".repeat(MAX_LOCALE_SOURCE_BYTES);
    let content = format!(r#"{{"greeting":"{oversized}"}}"#);
    fs::write(temp.path().join("en_US.json"), content).expect("write oversized locale");

    let err =
        DynamicJsonCatalog::from_directory(temp.path(), Locale::EN_US, FallbackStrategy::Both)
            .expect_err("oversized locale source should fail");

    assert!(matches!(
        err,
        DynamicCatalogError::LocaleSourceTooLarge {
            max_bytes,
            ..
        } if max_bytes == MAX_LOCALE_SOURCE_BYTES
    ));
}

#[test]
fn dynamic_catalog_rejects_excessive_total_source_bytes() {
    let payload = "x".repeat(MAX_LOCALE_SOURCE_BYTES / 2);
    let template = format!(r#"{{"greeting":"{payload}"}}"#);
    let source_len = template.len();

    let mut total_bytes = 0usize;
    let mut sources = Vec::new();
    let mut index = 0usize;
    while total_bytes <= MAX_CATALOG_TOTAL_BYTES {
        let locale = generated_locale(index);
        total_bytes += source_len;
        sources.push((format!("{locale}.json"), template.clone()));
        index += 1;
    }

    let default_locale =
        Locale::parse_canonical(&generated_locale(0)).expect("generated default locale");
    let err =
        DynamicJsonCatalog::from_locale_sources(sources, default_locale, FallbackStrategy::Both)
            .expect_err("catalog total size should be capped");

    assert!(matches!(
        err,
        DynamicCatalogError::CatalogTooLarge {
            max_bytes,
            ..
        } if max_bytes == MAX_CATALOG_TOTAL_BYTES
    ));
}

#[test]
fn dynamic_catalog_rejects_excessive_locale_source_count() {
    let sources = (0..=MAX_LOCALE_SOURCES)
        .map(|index| {
            let locale = generated_locale(index);
            (format!("{locale}.json"), "{}".to_string())
        })
        .collect::<Vec<_>>();
    let default_locale =
        Locale::parse_canonical(&generated_locale(0)).expect("generated default locale");

    let err =
        DynamicJsonCatalog::from_locale_sources(sources, default_locale, FallbackStrategy::Both)
            .expect_err("catalog source count should be capped");

    assert!(matches!(
        err,
        DynamicCatalogError::TooManyLocaleSources { max } if max == MAX_LOCALE_SOURCES
    ));
}
