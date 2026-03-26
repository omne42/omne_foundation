use super::locale_resolution::{
    LocaleRequest, resolve_environment_locale_request, select_locale_request,
};
use super::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use structured_text_kit::{CatalogTextRef, StructuredText, try_structured_text};

fn catalog_text(text: &StructuredText) -> CatalogTextRef<'_> {
    text.as_catalog()
        .expect("test text should be catalog-backed")
}

fn assert_catalog_text_arg(text: &StructuredText, name: &str, expected: Option<&str>) {
    assert_eq!(catalog_text(text).text_arg(name), expected);
}

#[derive(Debug)]
struct TestCatalog {
    by_locale: BTreeMap<Locale, BTreeMap<&'static str, &'static str>>,
    default_locale: Locale,
}

impl TranslationCatalog for TestCatalog {
    fn resolve_shared(&self, locale: Locale, key: &str) -> TranslationResolution {
        self.by_locale
            .get(&locale)
            .and_then(|messages| messages.get(key).copied())
            .map(Arc::<str>::from)
            .map_or(TranslationResolution::Missing, TranslationResolution::Exact)
    }
}

impl Catalog for TestCatalog {
    fn default_locale(&self) -> Locale {
        self.default_locale
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.by_locale.keys().copied().collect()
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.by_locale.contains_key(&locale)
    }
}

fn render_locale_error<C>(catalog: &C, error: ResolveLocaleError) -> String
where
    C: Catalog + ?Sized,
{
    error.render(catalog, catalog.default_locale())
}

#[test]
fn renders_template_interpolation() {
    let mut by_locale = BTreeMap::new();
    by_locale.insert(
        Locale::EN_US,
        BTreeMap::from([("greeting", "hello {name}")]),
    );
    by_locale.insert(
        Locale::ZH_CN,
        BTreeMap::from([("greeting", "你好，{name}")]),
    );

    let catalog = TestCatalog {
        by_locale,
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        catalog.render_text(
            Locale::ZH_CN,
            "greeting",
            &[TemplateArg::new("name", "Alice")],
        ),
        "你好，Alice"
    );
}

#[test]
fn render_text_returns_missing_key_without_interpolation() {
    let sources = [StaticJsonLocale::new(Locale::EN_US, true, "{}")];
    let catalog = StaticJsonCatalog::try_new(Locale::EN_US, &sources).expect("valid catalog");

    assert_eq!(
        catalog.render_text(
            Locale::EN_US,
            "missing.{name}",
            &[TemplateArg::new("name", "Alice")],
        ),
        "missing.{name}"
    );
}

#[test]
fn render_text_leaves_synthetic_key_uninterpolated() {
    let catalog = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": {} }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("valid catalog");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "missing.{name}"),
        Some("missing.{name}".to_string())
    );
    assert_eq!(
        catalog.render_text(
            Locale::EN_US,
            "missing.{name}",
            &[TemplateArg::new("name", "Alice")],
        ),
        "missing.{name}"
    );
}

#[test]
fn interpolation_does_not_reinterpret_inserted_values() {
    assert_eq!(
        interpolate(
            "{name}",
            &[
                TemplateArg::new("name", "{role}"),
                TemplateArg::new("role", "admin"),
            ],
        ),
        "{role}"
    );
}

#[test]
fn interpolation_prefers_later_duplicate_args() {
    assert_eq!(
        interpolate(
            "{name}",
            &[
                TemplateArg::new("name", "Alice"),
                TemplateArg::new("name", "Bob"),
            ],
        ),
        "Bob"
    );
}

#[test]
fn interpolation_leaves_unknown_placeholders_literal() {
    assert_eq!(
        interpolate("hello {name}", &[TemplateArg::new("role", "admin")]),
        "hello {name}"
    );
}

#[test]
fn interpolation_leaves_unclosed_placeholders_literal() {
    assert_eq!(
        interpolate("hello {name", &[TemplateArg::new("name", "Alice")]),
        "hello {name"
    );
}

#[test]
fn renders_nested_structured_text_with_catalog() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(
            Locale::EN_US,
            BTreeMap::from([("outer", "outer: {child}"), ("inner", "hello {name}")]),
        )]),
        default_locale: Locale::EN_US,
    };
    let inner = try_structured_text!("inner", "name" => "friend")
        .expect("literal arg name should be valid");
    let text =
        try_structured_text!("outer", "child" => @text inner).expect("valid structured text");

    assert_eq!(
        render_structured_text(&catalog, Locale::EN_US, &text),
        "outer: hello friend"
    );
}

#[test]
fn renders_freeform_structured_text_as_raw_text() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(
            Locale::EN_US,
            BTreeMap::from([("error_detail.freeform", "should not be used")]),
        )]),
        default_locale: Locale::EN_US,
    };
    let text = StructuredText::freeform("plain text");

    assert_eq!(
        render_structured_text(&catalog, Locale::EN_US, &text),
        "plain text"
    );
}

#[test]
fn locale_parse_normalizes_valid_locale_ids() {
    assert_eq!(
        Locale::parse("en").map(|locale| locale.to_string()),
        Some("en".to_string())
    );
    assert_eq!(
        Locale::parse("fr-FR").map(|locale| locale.to_string()),
        Some("fr_FR".to_string())
    );
    assert_eq!(
        Locale::parse("zh-Hant-TW").map(|locale| locale.to_string()),
        Some("zh_Hant_TW".to_string())
    );
}

#[test]
fn locale_parse_system_normalizes_system_locale_ids() {
    assert_eq!(
        Locale::parse_system("en_US.UTF-8").map(|locale| locale.to_string()),
        Some("en_US".to_string())
    );
    assert_eq!(
        Locale::parse_system("en_US.ISO-8859-1").map(|locale| locale.to_string()),
        Some("en_US".to_string())
    );
    assert_eq!(
        Locale::parse_system("ja_JP.EUC-JP").map(|locale| locale.to_string()),
        Some("ja_JP".to_string())
    );
    assert_eq!(
        Locale::parse_system("fr_FR@euro").map(|locale| locale.to_string()),
        Some("fr_FR".to_string())
    );
    assert_eq!(
        Locale::parse_system("ca_ES@valencia").map(|locale| locale.to_string()),
        Some("ca_ES".to_string())
    );
    assert_eq!(
        Locale::parse_system("ca_ES.UTF-8@valencia").map(|locale| locale.to_string()),
        Some("ca_ES".to_string())
    );
    assert_eq!(
        Locale::parse_system("sr_RS@latin").map(|locale| locale.to_string()),
        Some("sr_Latn_RS".to_string())
    );
    assert_eq!(
        Locale::parse_system("sr_RS@cyrillic").map(|locale| locale.to_string()),
        Some("sr_Cyrl_RS".to_string())
    );
}

#[test]
fn locale_parse_system_ignores_unknown_modifiers() {
    assert_eq!(
        Locale::parse_system("de_DE@phonebook").map(|locale| locale.to_string()),
        Some("de_DE".to_string())
    );
    assert_eq!(
        Locale::parse_system("de_DE.UTF-8@phonebook").map(|locale| locale.to_string()),
        Some("de_DE".to_string())
    );
}

#[test]
fn locale_parse_system_ignores_inapplicable_script_modifiers() {
    assert_eq!(
        Locale::parse_system("en_US@traditional").map(|locale| locale.to_string()),
        Some("en_US".to_string())
    );
    assert_eq!(
        Locale::parse_system("sr_RS@traditional").map(|locale| locale.to_string()),
        Some("sr_RS".to_string())
    );
}

#[test]
fn locale_parse_system_rejects_invalid_modifier_characters() {
    assert_eq!(Locale::parse_system("en_US@latin!"), None);
    assert_eq!(Locale::parse_system("en_US@foo.bar"), None);
}

#[test]
fn locale_parse_system_rejects_conflicting_script_modifiers() {
    assert_eq!(Locale::parse_system("sr_Cyrl_RS@latin"), None);
    assert_eq!(Locale::parse_system("zh_Hant_TW@simplified"), None);
}

#[test]
fn locale_parse_rejects_overlong_locale_ids() {
    let too_long = format!("{}-{}", "x".repeat(40), "y".repeat(40));

    assert_eq!(Locale::parse(&too_long), None);
}

#[test]
fn locale_parse_rejects_invalid_locale_shapes() {
    assert_eq!(Locale::parse("definitely-not-a-locale"), None);
    assert_eq!(Locale::parse("en-XYZ"), None);
    assert_eq!(Locale::parse("hant-TW"), None);
    assert_eq!(Locale::parse("en__US"), None);
    assert_eq!(Locale::parse("_en_US"), None);
    assert_eq!(Locale::parse("en_US_"), None);
    assert_eq!(Locale::parse("en@foo_US"), None);
    assert_eq!(Locale::parse("en_US@foo.bar"), None);
    assert_eq!(Locale::parse("en-US.more@x"), None);
}

#[test]
fn locale_parse_canonical_rejects_noncanonical_input() {
    assert_eq!(Locale::parse_canonical("fr-FR"), None);
    assert_eq!(Locale::parse_canonical("en_US.UTF-8"), None);
    assert_eq!(Locale::parse_canonical("en_US@formal"), None);
}

#[test]
fn locale_from_static_accepts_canonical_locale_ids() {
    assert_eq!(Locale::from_static("fr_FR").to_string(), "fr_FR");
    assert_eq!(Locale::from_static("zh_Hant_TW").to_string(), "zh_Hant_TW");
    assert_eq!(Locale::from_static("es_419").to_string(), "es_419");
}

#[test]
#[should_panic(expected = "locale must use canonical language[_Script][_REGION] form")]
fn locale_from_static_rejects_non_canonical_locale_ids() {
    let _ = Locale::from_static("en-us");
}

#[test]
fn static_json_catalog_falls_back_to_default_locale() {
    static SOURCES: [StaticJsonLocale; 2] = [
        StaticJsonLocale::new(Locale::EN_US, true, r#"{"greeting":"hello {name}"}"#),
        StaticJsonLocale::new(Locale::ZH_CN, false, r#"{"greeting":"你好，{name}"}"#),
    ];
    static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
        StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
    });

    assert_eq!(CATALOG.default_locale(), Locale::EN_US);
    assert_eq!(CATALOG.available_locales(), vec![Locale::EN_US]);
    assert_eq!(
        CATALOG.render_text(
            Locale::JA_JP,
            "greeting",
            &[TemplateArg::new("name", "Alice")],
        ),
        "hello Alice"
    );
}

#[test]
fn resolve_environment_locale_request_uses_system_locale_normalization() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([
            (
                Locale::from_static("sr_Latn_RS"),
                BTreeMap::from([("greeting", "latin")]),
            ),
            (Locale::EN_US, BTreeMap::from([("greeting", "hello")])),
        ]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        resolve_environment_locale_request(&catalog, "sr_RS@latin")
            .expect("system locale should map to script-aware locale"),
        Locale::from_static("sr_Latn_RS")
    );
}

#[test]
fn resolve_environment_locale_request_does_not_invent_script_locale_from_modifier() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([
            (Locale::EN_US, BTreeMap::from([("greeting", "hello")])),
            (
                Locale::from_static("en_Hant"),
                BTreeMap::from([("greeting", "invented")]),
            ),
        ]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        resolve_environment_locale_request(&catalog, "en_US@traditional")
            .expect("inapplicable modifier should fall back to the base locale"),
        Locale::EN_US
    );
}

#[test]
fn resolve_environment_locale_request_falls_back_to_default_for_unavailable_locale() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(Locale::EN_US, BTreeMap::from([("greeting", "hello")]))]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        resolve_environment_locale_request(&catalog, "es_MX")
            .expect("unavailable environment locale should fall back to default"),
        Locale::EN_US
    );
}

#[test]
fn resolve_environment_locale_request_ignores_invalid_system_locale() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(Locale::EN_US, BTreeMap::from([("greeting", "hello")]))]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        resolve_environment_locale_request(&catalog, "not a locale")
            .expect("invalid environment locale should fall back to default"),
        Locale::EN_US
    );
}

#[test]
fn resolve_locale_treats_whitespace_explicit_request_as_default() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(Locale::EN_US, BTreeMap::from([("greeting", "hello")]))]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("   "))
            .expect("whitespace explicit locale should behave like no explicit locale"),
        Locale::EN_US
    );
}

#[test]
fn select_locale_request_ignores_posix_env_locale() {
    assert_eq!(select_locale_request(None, Some("C.UTF-8")), None);
    assert_eq!(select_locale_request(None, Some("C.ISO-8859-1")), None);
    assert_eq!(select_locale_request(None, Some("POSIX")), None);
    assert_eq!(select_locale_request(None, Some("POSIX.ASCII")), None);
    assert_eq!(select_locale_request(None, Some("C.UTF-8@x")), None);
    assert_eq!(
        select_locale_request(None, Some("fr_FR")),
        Some(LocaleRequest::Environment("fr_FR"))
    );
    assert_eq!(
        select_locale_request(Some("zh_CN"), Some("C.UTF-8")),
        Some(LocaleRequest::Explicit("zh_CN"))
    );
}

#[test]
fn resolve_locale_falls_back_to_parent_locale() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(
            Locale::from_static("fr"),
            BTreeMap::from([("greeting", "bonjour")]),
        )]),
        default_locale: Locale::from_static("fr"),
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("fr_FR"))
            .expect("fr_FR fallback"),
        Locale::from_static("fr")
    );
}

#[test]
fn resolve_locale_prefers_script_parent_before_region_parent() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([
            (
                Locale::from_static("sr_Latn"),
                BTreeMap::from([("greeting", "latin")]),
            ),
            (
                Locale::from_static("sr_RS"),
                BTreeMap::from([("greeting", "region")]),
            ),
        ]),
        default_locale: Locale::from_static("sr_Latn"),
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("sr_Latn_RS"))
            .expect("sr_Latn_RS should prefer sr_Latn"),
        Locale::from_static("sr_Latn")
    );
}

#[test]
fn resolve_locale_falls_back_to_region_parent_when_script_parent_is_absent() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(
            Locale::from_static("sr_RS"),
            BTreeMap::from([("greeting", "region")]),
        )]),
        default_locale: Locale::from_static("sr_RS"),
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("sr_Latn_RS"))
            .expect("sr_Latn_RS should fall back to sr_RS"),
        Locale::from_static("sr_RS")
    );
}

#[test]
fn resolve_locale_rejects_posix_pseudo_locale_requests() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(Locale::EN_US, BTreeMap::from([("greeting", "hello")]))]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        catalog.try_resolve_locale(Some("C.UTF-8")),
        Err(ResolveLocaleError::UnknownLocale {
            requested: "C.UTF-8".to_string(),
        })
    );
    let error = catalog
        .try_resolve_locale(Some("POSIX"))
        .expect_err("POSIX should be rejected as an explicit locale request");
    assert_eq!(
        render_locale_error(&catalog, error),
        "unknown locale identifier: POSIX"
    );
}

#[test]
fn resolve_locale_falls_back_to_likely_chinese_script() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(
            Locale::from_static("zh_Hant"),
            BTreeMap::from([("greeting", "traditional")]),
        )]),
        default_locale: Locale::from_static("zh_Hant"),
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("zh_TW"))
            .expect("zh_TW should resolve to zh_Hant"),
        Locale::from_static("zh_Hant")
    );
}

#[test]
fn resolve_locale_prefers_likely_chinese_script_before_generic_language() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([
            (
                Locale::from_static("zh"),
                BTreeMap::from([("greeting", "generic")]),
            ),
            (
                Locale::from_static("zh_Hant"),
                BTreeMap::from([("greeting", "traditional")]),
            ),
        ]),
        default_locale: Locale::from_static("zh"),
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("zh_TW"))
            .expect("zh_TW should prefer zh_Hant over zh"),
        Locale::from_static("zh_Hant")
    );
}

#[test]
fn resolve_locale_does_not_invent_region_defaults() {
    let catalog = TestCatalog {
        by_locale: BTreeMap::from([(Locale::EN_US, BTreeMap::from([("greeting", "hello")]))]),
        default_locale: Locale::EN_US,
    };

    assert_eq!(
        catalog.try_resolve_locale(Some("en")),
        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: "en".to_string(),
            available: vec![Locale::EN_US],
        })
    );
    let error = catalog
        .try_resolve_locale(Some("en"))
        .expect_err("en should stay disabled");
    assert_eq!(
        render_locale_error(&catalog, error),
        "locale is not enabled: en; available locales: en_US"
    );
}

#[test]
fn resolve_locale_none_requires_default_locale_to_be_enabled() {
    let catalog = DynamicJsonCatalog::empty(Locale::EN_US, FallbackStrategy::ReturnKey);

    assert_eq!(
        catalog.try_resolve_locale(None),
        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: "en_US".to_string(),
            available: Vec::new(),
        })
    );
    let error = catalog
        .try_resolve_locale(None)
        .expect_err("default locale should stay disabled");
    assert_eq!(
        render_locale_error(&catalog, error),
        "locale is not enabled: en_US; no locales are available"
    );
}

#[test]
fn empty_composed_catalog_does_not_resolve_unavailable_default_locale() {
    let catalog = ComposedCatalog::new(Locale::EN_US);

    assert_eq!(
        catalog.try_resolve_locale(None),
        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: "en_US".to_string(),
            available: Vec::new(),
        })
    );
}

#[test]
fn resolve_locale_uses_single_available_locale_snapshot() {
    struct FlakyLocaleEnabledCatalog {
        enabled_locales: Vec<Locale>,
        parent_locale_rejected_once: Mutex<bool>,
    }

    impl TranslationCatalog for FlakyLocaleEnabledCatalog {
        fn resolve_shared(&self, _locale: Locale, _key: &str) -> TranslationResolution {
            TranslationResolution::Missing
        }
    }

    impl Catalog for FlakyLocaleEnabledCatalog {
        fn lookup_shared(&self, _locale: Locale, _key: &str) -> Option<Arc<str>> {
            None
        }

        fn default_locale(&self) -> Locale {
            Locale::EN_US
        }

        fn available_locales(&self) -> Vec<Locale> {
            self.enabled_locales.clone()
        }

        fn locale_enabled(&self, locale: Locale) -> bool {
            let parent = Locale::from_static("fr");
            if locale == parent {
                let mut rejected = self
                    .parent_locale_rejected_once
                    .lock()
                    .expect("lock flaky locale state");
                if !*rejected {
                    *rejected = true;
                    return false;
                }
            }

            self.enabled_locales.contains(&locale)
        }
    }

    let catalog = FlakyLocaleEnabledCatalog {
        enabled_locales: vec![Locale::from_static("fr")],
        parent_locale_rejected_once: Mutex::new(false),
    };

    assert_eq!(
        catalog
            .try_resolve_locale(Some("fr_FR"))
            .expect("resolution should use the snapshot of available locales"),
        Locale::from_static("fr")
    );
}

#[test]
fn resolve_locale_error_render_falls_back_to_display_for_synthetic_keys() {
    let catalog = ComposedCatalog::new(Locale::EN_US).add_catalog(
        DynamicJsonCatalog::from_json_string(
            r#"{ "en_US": { "greeting": "hello" } }"#,
            Locale::EN_US,
            FallbackStrategy::ReturnKey,
        )
        .expect("valid catalog"),
    );

    let rendered = ResolveLocaleError::UnknownLocale {
        requested: "xx_YY".to_string(),
    }
    .render(&catalog, Locale::EN_US);

    assert_eq!(rendered, "unknown locale identifier: xx_YY");
}

#[test]
fn resolve_locale_error_render_preserves_identity_translations() {
    let catalog = DynamicJsonCatalog::from_json_string(
        r#"{ "en_US": { "locale.unknown": "locale.unknown" } }"#,
        Locale::EN_US,
        FallbackStrategy::ReturnKey,
    )
    .expect("valid catalog");

    let rendered = ResolveLocaleError::UnknownLocale {
        requested: "xx_YY".to_string(),
    }
    .render(&catalog, Locale::EN_US);

    assert_eq!(rendered, "locale.unknown");
}

#[test]
fn resolve_locale_error_exposes_catalog_backed_structured_text() {
    let text = ResolveLocaleError::UnknownLocale {
        requested: "xx_YY".to_string(),
    }
    .to_structured_text();

    assert_eq!(catalog_text(&text).code(), "locale.unknown");
    assert_catalog_text_arg(&text, "requested", Some("xx_YY"));
}

#[test]
fn composed_catalog_keeps_searching_after_synthetic_key_fallbacks() {
    struct SyntheticFallbackCatalog;

    impl TranslationCatalog for SyntheticFallbackCatalog {
        fn resolve_shared(&self, _locale: Locale, key: &str) -> TranslationResolution {
            TranslationResolution::Synthetic(Arc::<str>::from(key))
        }
    }

    impl Catalog for SyntheticFallbackCatalog {
        fn lookup_shared(&self, _locale: Locale, _key: &str) -> Option<Arc<str>> {
            None
        }

        fn default_locale(&self) -> Locale {
            Locale::EN_US
        }

        fn available_locales(&self) -> Vec<Locale> {
            vec![Locale::EN_US]
        }
    }

    let composed = ComposedCatalog::new(Locale::EN_US)
        .add_catalog(SyntheticFallbackCatalog)
        .add_catalog(
            StaticJsonCatalog::try_new(
                Locale::EN_US,
                &[StaticJsonLocale::new(
                    Locale::EN_US,
                    true,
                    r#"{"greeting":"hello"}"#,
                )],
            )
            .expect("valid catalog"),
        );

    assert_eq!(
        composed.get_text(Locale::EN_US, "greeting"),
        Some("hello".to_string())
    );
}

#[test]
fn static_json_catalog_rejects_duplicate_enabled_locale_sources() {
    static SOURCES: [StaticJsonLocale; 2] = [
        StaticJsonLocale::new(Locale::EN_US, true, r#"{"hello":"hello"}"#),
        StaticJsonLocale::new(Locale::EN_US, true, r#"{"hello":"hi"}"#),
    ];
    let error = StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES)
        .expect_err("duplicate locales should fail");
    assert!(matches!(
        error,
        StaticCatalogError::DuplicateEnabledLocale(Locale::EN_US)
    ));
}

#[test]
fn static_json_catalog_requires_enabled_default_locale() {
    static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
        Locale::EN_US,
        false,
        r#"{"hello":"hello"}"#,
    )];
    let error = StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES)
        .expect_err("disabled default locale should fail");
    assert!(matches!(
        error,
        StaticCatalogError::MissingDefaultLocale(Locale::EN_US)
    ));
}

#[test]
fn static_json_catalog_rejects_invalid_json_at_construction() {
    static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
        Locale::EN_US,
        true,
        r#"{"hello":"world""#,
    )];
    let error =
        StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect_err("invalid JSON should fail");
    assert!(matches!(
        error,
        StaticCatalogError::InvalidLocaleJson {
            locale: Locale::EN_US,
            ..
        }
    ));
}

#[test]
fn static_json_catalog_rejects_invalid_templates_at_construction() {
    static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
        Locale::EN_US,
        true,
        r#"{"hello":"hello {name"}"#,
    )];
    let error = StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES)
        .expect_err("invalid template should fail");
    match error {
        StaticCatalogError::InvalidLocaleJson { locale, error } => {
            assert_eq!(locale, Locale::EN_US);
            assert!(
                error
                    .to_string()
                    .contains("invalid catalog template for hello: unclosed placeholder")
            );
        }
        other => panic!("expected invalid locale json error, got {other}"),
    }
}

#[test]
fn static_json_catalog_accepts_non_static_source_slice() {
    let sources = vec![StaticJsonLocale::new(
        Locale::EN_US,
        true,
        r#"{"hello":"hello"}"#,
    )];

    let catalog = StaticJsonCatalog::try_new(Locale::EN_US, &sources).expect("valid catalog");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "hello"),
        Some("hello".to_string())
    );
}

#[test]
fn static_json_catalog_try_new_supports_lazy_static_construction() {
    static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
        Locale::EN_US,
        true,
        r#"{"hello":"hello"}"#,
    )];
    static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
        StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
    });

    assert_eq!(
        CATALOG.get_text(Locale::EN_US, "hello"),
        Some("hello".to_string())
    );
}

#[test]
fn static_json_catalog_macro_returns_result() {
    let catalog = static_json_catalog! {
        default: Locale::EN_US,
        Locale::EN_US => {
            enabled: true,
            json: r#"{"hello":"hello"}"#
        },
    }
    .expect("valid macro catalog");

    assert_eq!(
        catalog.get_text(Locale::EN_US, "hello"),
        Some("hello".to_string())
    );

    let error = static_json_catalog! {
        default: Locale::EN_US,
        Locale::EN_US => {
            enabled: true,
            json: r#"{"hello":"hello {name"}"#
        },
    }
    .expect_err("invalid macro sources should return an error");

    assert!(matches!(
        error,
        StaticCatalogError::InvalidLocaleJson {
            locale: Locale::EN_US,
            ..
        }
    ));
}
