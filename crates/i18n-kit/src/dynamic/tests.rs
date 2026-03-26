use super::locale_sources::{MAX_CATALOG_TOTAL_BYTES, MAX_LOCALE_SOURCE_BYTES, MAX_LOCALE_SOURCES};
use super::*;
use crate::{Catalog, TranslationCatalog, TranslationResolution};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

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
