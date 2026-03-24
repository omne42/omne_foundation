use i18n_kit::{
    DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy, Locale, TemplateArg,
    TranslationCatalog,
};
use proptest::prelude::*;
use proptest::string::string_regex;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
struct LocaleParts {
    language: String,
    script: Option<String>,
    region: Option<String>,
}

impl LocaleParts {
    fn canonical(&self) -> String {
        build_locale_string(
            self.language.as_str(),
            self.script.as_deref(),
            self.region.as_deref(),
            '_',
        )
    }

    fn with_script(&self, script: &str) -> String {
        build_locale_string(
            self.language.as_str(),
            Some(script),
            self.region.as_deref(),
            '_',
        )
    }
}

#[derive(Debug, Clone)]
struct TemplateCase {
    key: String,
    template: String,
    args: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct InvalidTemplateCase {
    key: String,
    template: String,
}

fn build_locale_string(
    language: &str,
    script: Option<&str>,
    region: Option<&str>,
    separator: char,
) -> String {
    let mut value = String::with_capacity(
        language.len()
            + script.map_or(0, |part| part.len() + 1)
            + region.map_or(0, |part| part.len() + 1),
    );
    value.push_str(language);
    if let Some(script) = script {
        value.push(separator);
        value.push_str(script);
    }
    if let Some(region) = region {
        value.push(separator);
        value.push_str(region);
    }
    value
}

fn locale_json(key: &str, template: &str) -> String {
    let mut texts = Map::new();
    texts.insert(key.to_owned(), json!(template));

    let mut locales = Map::new();
    locales.insert("en_US".to_owned(), Value::Object(texts));

    Value::Object(locales).to_string()
}

fn render_reference(template: &str, args: &[(String, String)]) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        rendered.push_str(&rest[..start]);
        let placeholder_tail = &rest[start + 1..];
        let end = placeholder_tail
            .find('}')
            .expect("generated valid templates always close placeholders");
        let name = &placeholder_tail[..end];
        if let Some((_, value)) = args.iter().rev().find(|(arg_name, _)| arg_name == name) {
            rendered.push_str(value);
        } else {
            rendered.push('{');
            rendered.push_str(name);
            rendered.push('}');
        }
        rest = &placeholder_tail[end + 1..];
    }

    rendered.push_str(rest);
    rendered
}

fn language_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        string_regex("[a-z]{2}").expect("language regex must compile"),
        string_regex("[a-z]{3}").expect("language regex must compile"),
    ]
}

fn script_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Latn".to_owned()),
        Just("Cyrl".to_owned()),
        Just("Hans".to_owned()),
        Just("Hant".to_owned()),
        Just("Arab".to_owned()),
        Just("Deva".to_owned()),
    ]
}

fn region_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        string_regex("[A-Z]{2}").expect("region regex must compile"),
        string_regex("[0-9]{3}").expect("region regex must compile"),
    ]
}

fn locale_parts_strategy() -> impl Strategy<Value = LocaleParts> {
    (
        language_strategy(),
        prop::option::of(script_strategy()),
        prop::option::of(region_strategy()),
    )
        .prop_map(|(language, script, region)| LocaleParts {
            language,
            script,
            region,
        })
}

fn language_case_options(language: &str) -> Vec<String> {
    vec![
        language.to_owned(),
        language.to_ascii_uppercase(),
        format!("{}{}", language[..1].to_ascii_uppercase(), &language[1..]),
    ]
}

fn script_case_options(script: Option<&str>) -> Vec<Option<String>> {
    match script {
        Some(script) => vec![
            Some(script.to_owned()),
            Some(script.to_ascii_lowercase()),
            Some(script.to_ascii_uppercase()),
        ],
        None => vec![None],
    }
}

fn region_case_options(region: Option<&str>) -> Vec<Option<String>> {
    match region {
        Some(region) if region.as_bytes().iter().all(u8::is_ascii_alphabetic) => {
            vec![Some(region.to_owned()), Some(region.to_ascii_lowercase())]
        }
        Some(region) => vec![Some(region.to_owned())],
        None => vec![None],
    }
}

fn locale_variant_strategy() -> impl Strategy<Value = (String, String)> {
    locale_parts_strategy()
        .prop_flat_map(|parts| {
            (
                Just(parts.canonical()),
                prop::sample::select(language_case_options(parts.language.as_str())),
                prop::sample::select(script_case_options(parts.script.as_deref())),
                prop::sample::select(region_case_options(parts.region.as_deref())),
                prop_oneof![Just('_'), Just('-')],
            )
        })
        .prop_map(|(expected, language, script, region, separator)| {
            (
                build_locale_string(
                    language.as_str(),
                    script.as_deref(),
                    region.as_deref(),
                    separator,
                ),
                expected,
            )
        })
}

fn codeset_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("UTF-8".to_owned()),
        Just("ISO-8859-1".to_owned()),
        Just("EUC-JP".to_owned()),
        string_regex("[A-Za-z0-9-]{1,10}").expect("codeset regex must compile"),
    ]
}

fn applicable_system_modifiers(parts: &LocaleParts) -> Vec<(String, Option<String>)> {
    let mut options = vec![
        (String::new(), None),
        ("euro".to_owned(), None),
        ("phonebook".to_owned(), None),
        ("valencia".to_owned(), None),
    ];

    match parts.language.as_str() {
        "sr" => {
            add_script_modifier_option(&mut options, &parts.script, "latin", "Latn");
            add_script_modifier_option(&mut options, &parts.script, "cyrillic", "Cyrl");
        }
        "zh" => {
            add_script_modifier_option(&mut options, &parts.script, "traditional", "Hant");
            add_script_modifier_option(&mut options, &parts.script, "simplified", "Hans");
        }
        _ => {}
    }

    options
}

fn add_script_modifier_option(
    options: &mut Vec<(String, Option<String>)>,
    script: &Option<String>,
    modifier: &str,
    alias: &str,
) {
    if script.as_deref().is_none() || script.as_deref() == Some(alias) {
        options.push((modifier.to_owned(), Some(alias.to_owned())));
    }
}

fn system_locale_strategy() -> impl Strategy<Value = (String, String)> {
    locale_parts_strategy()
        .prop_flat_map(|parts| {
            let modifiers = applicable_system_modifiers(&parts);
            (
                Just(parts),
                prop::option::of(codeset_strategy()),
                prop::sample::select(modifiers),
            )
        })
        .prop_map(|(parts, codeset, (modifier, alias_script))| {
            let mut system = parts.canonical();
            if let Some(codeset) = codeset {
                system.push('.');
                system.push_str(&codeset);
            }
            if !modifier.is_empty() {
                system.push('@');
                system.push_str(&modifier);
            }

            let expected = match (parts.script.as_deref(), alias_script.as_deref()) {
                (Some(_), _) | (_, None) => parts.canonical(),
                (None, Some(alias)) => parts.with_script(alias),
            };

            (system, expected)
        })
}

fn identifier_segment_strategy() -> impl Strategy<Value = String> {
    string_regex("[A-Za-z0-9_-]{1,6}").expect("identifier regex must compile")
}

fn identifier_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(identifier_segment_strategy(), 1..=4)
        .prop_map(|segments| segments.join("."))
}

fn literal_piece_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just(" ".to_owned()),
        Just("你好".to_owned()),
        string_regex("[A-Za-z0-9 .,_-]{1,8}").expect("literal regex must compile"),
    ]
}

fn value_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("{role}".to_owned()),
        Just("🙂".to_owned()),
        string_regex("[A-Za-z0-9 {}._-]{1,8}").expect("value regex must compile"),
    ]
}

fn template_case_strategy() -> impl Strategy<Value = TemplateCase> {
    (
        identifier_strategy(),
        prop::collection::vec(
            (
                literal_piece_strategy(),
                prop::option::of(identifier_strategy()),
            ),
            1..=6,
        ),
        prop::collection::vec((identifier_strategy(), value_strategy()), 0..=6),
    )
        .prop_map(|(key, parts, args)| {
            let mut template = String::new();
            for (literal, placeholder) in parts {
                template.push_str(&literal);
                if let Some(placeholder) = placeholder {
                    template.push('{');
                    template.push_str(&placeholder);
                    template.push('}');
                }
            }

            TemplateCase {
                key,
                template,
                args,
            }
        })
}

fn invalid_placeholder_strategy() -> impl Strategy<Value = String> {
    string_regex("[A-Za-z0-9_-]{1,4}![A-Za-z0-9_.-]{0,4}")
        .expect("invalid placeholder regex must compile")
}

fn invalid_template_case_strategy() -> impl Strategy<Value = InvalidTemplateCase> {
    (
        identifier_strategy(),
        literal_piece_strategy(),
        literal_piece_strategy(),
        identifier_strategy(),
        invalid_placeholder_strategy(),
    )
        .prop_flat_map(|(key, prefix, suffix, placeholder, invalid_placeholder)| {
            prop_oneof![
                Just(InvalidTemplateCase {
                    key: key.clone(),
                    template: format!("{prefix}{{{placeholder}{suffix}"),
                }),
                Just(InvalidTemplateCase {
                    key: key.clone(),
                    template: format!("{prefix}}}{suffix}"),
                }),
                Just(InvalidTemplateCase {
                    key: key.clone(),
                    template: format!("{prefix}{{}}{suffix}"),
                }),
                Just(InvalidTemplateCase {
                    key,
                    template: format!("{prefix}{{{invalid_placeholder}}}{suffix}"),
                }),
            ]
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn parse_normalizes_generated_locale_variants((input, expected) in locale_variant_strategy()) {
        let parsed = Locale::parse(&input);
        let canonical = Locale::parse_canonical(&expected);

        prop_assert_eq!(parsed.map(|locale| locale.to_string()), Some(expected.clone()));
        prop_assert_eq!(canonical.map(|locale| locale.to_string()), Some(expected));
    }

    #[test]
    fn parse_system_normalizes_generated_system_locales((input, expected) in system_locale_strategy()) {
        let parsed = Locale::parse_system(&input);

        prop_assert_eq!(parsed.map(|locale| locale.to_string()), Some(expected));
    }

    #[test]
    fn generated_valid_templates_render_like_reference(case in template_case_strategy()) {
        let json = locale_json(case.key.as_str(), case.template.as_str());
        let catalog = DynamicJsonCatalog::from_json_string(
            &json,
            Locale::EN_US,
            FallbackStrategy::Both,
        );
        prop_assert!(catalog.is_ok(), "generated template should be valid: {case:?}");
        let catalog = catalog.expect("catalog must load after successful assertion");

        let args: Vec<_> = case
            .args
            .iter()
            .map(|(name, value)| TemplateArg::new(name.as_str(), value.as_str()))
            .collect();
        let expected = render_reference(case.template.as_str(), &case.args);

        prop_assert_eq!(
            catalog.render_text(Locale::EN_US, case.key.as_str(), &args),
            expected,
        );
    }

    #[test]
    fn generated_invalid_templates_are_rejected(case in invalid_template_case_strategy()) {
        let json = locale_json(case.key.as_str(), case.template.as_str());
        let result = DynamicJsonCatalog::from_json_string(
            &json,
            Locale::EN_US,
            FallbackStrategy::Both,
        );

        prop_assert!(
            matches!(result, Err(DynamicCatalogError::LocaleSourceJson { .. })),
            "generated invalid template should be rejected: {case:?}",
        );
    }
}
