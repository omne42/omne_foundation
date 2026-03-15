use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::{Arc, OnceLock, RwLock};

pub mod dynamic;

pub use dynamic::{ComposedCatalog, DynamicCatalogError, DynamicJsonCatalog, FallbackStrategy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageArg<'a> {
    name: &'static str,
    value: Cow<'a, str>,
}

impl<'a> MessageArg<'a> {
    #[must_use]
    pub fn new(name: &'static str, value: impl Into<Cow<'a, str>>) -> Self {
        Self {
            name,
            value: value.into(),
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn value(&self) -> &str {
        self.value.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Locale(&'static str);

#[allow(non_upper_case_globals)]
impl Locale {
    pub const EnUs: Self = Self("en_US");
    pub const ZhCn: Self = Self("zh_CN");
    pub const JaJp: Self = Self("ja_JP");

    #[must_use]
    pub const fn from_static(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = normalize_locale_id(value)?;
        match normalized.as_str() {
            "en" | "en_US" => Some(Self::EnUs),
            "zh" | "zh_CN" | "zh_Hans" | "zh_Hans_CN" => Some(Self::ZhCn),
            "ja" | "ja_JP" => Some(Self::JaJp),
            other => Some(Self(Box::leak(other.to_string().into_boxed_str()))),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Display for Locale {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait MessageCatalog: Send + Sync {
    fn get(&self, locale: Locale, key: &str) -> Option<String>;
}

pub trait Catalog: MessageCatalog {
    fn default_locale(&self) -> Locale;

    fn available_locales(&self) -> Vec<Locale>;

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.available_locales().contains(&locale)
    }

    fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, String> {
        let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(self.default_locale());
        };

        let locale = Locale::parse(requested).ok_or_else(|| {
            render_message(
                self,
                self.default_locale(),
                "locale.unknown",
                &[MessageArg::new("requested", requested)],
            )
        })?;
        if self.locale_enabled(locale) {
            return Ok(locale);
        }

        let available = self
            .available_locales()
            .into_iter()
            .map(Locale::as_str)
            .collect::<Vec<_>>();
        if available.is_empty() {
            return Err(render_message(
                self,
                self.default_locale(),
                "locale.not_enabled.none",
                &[MessageArg::new("requested", requested)],
            ));
        }

        Err(render_message(
            self,
            self.default_locale(),
            "locale.not_enabled.available",
            &[
                MessageArg::new("requested", requested),
                MessageArg::new("available", available.join(", ")),
            ],
        ))
    }

    fn resolve_cli_locale(
        &self,
        args: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), String> {
        let (requested_locale, args) = strip_locale_args(self, args)?;
        let env_locale = std::env::var(env_var).ok();
        let locale = self.resolve_locale(requested_locale.as_deref().or(env_locale.as_deref()))?;
        Ok((locale, args))
    }
}

pub trait MessageCatalogExt: MessageCatalog {
    #[must_use]
    fn render(&self, locale: Locale, key: &str, args: &[MessageArg<'_>]) -> String {
        render_message(self, locale, key, args)
    }
}

impl<T> MessageCatalogExt for T where T: MessageCatalog + ?Sized {}

#[derive(Debug, Clone, Copy)]
pub struct StaticJsonLocale {
    pub locale: Locale,
    pub enabled: bool,
    pub json: &'static str,
}

impl StaticJsonLocale {
    #[must_use]
    pub const fn new(locale: Locale, enabled: bool, json: &'static str) -> Self {
        Self {
            locale,
            enabled,
            json,
        }
    }
}

#[derive(Debug)]
pub struct StaticJsonCatalog {
    default_locale: Locale,
    locales: &'static [StaticJsonLocale],
    parsed: OnceLock<BTreeMap<Locale, BTreeMap<String, String>>>,
}

impl StaticJsonCatalog {
    #[must_use]
    pub const fn new(default_locale: Locale, locales: &'static [StaticJsonLocale]) -> Self {
        Self {
            default_locale,
            locales,
            parsed: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn default_locale(&self) -> Locale {
        Catalog::default_locale(self)
    }

    #[must_use]
    pub fn available_locales(&self) -> Vec<Locale> {
        Catalog::available_locales(self)
    }

    #[must_use]
    pub fn locale_enabled(&self, locale: Locale) -> bool {
        Catalog::locale_enabled(self, locale)
    }

    pub fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, String> {
        Catalog::resolve_locale(self, requested)
    }

    pub fn resolve_cli_locale(
        &self,
        args: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), String> {
        Catalog::resolve_cli_locale(self, args, env_var)
    }

    #[must_use]
    pub fn compiled_locales(&self) -> Vec<Locale> {
        self.locales
            .iter()
            .filter(|source| source.enabled)
            .map(|source| source.locale)
            .collect()
    }

    fn parsed_locales(&self) -> &BTreeMap<Locale, BTreeMap<String, String>> {
        self.parsed.get_or_init(|| {
            self.locales
                .iter()
                .filter(|source| source.enabled)
                .map(|source| {
                    let messages =
                        serde_json::from_str(source.json).expect("i18n catalog JSON must be valid");
                    (source.locale, messages)
                })
                .collect()
        })
    }
}

impl MessageCatalog for StaticJsonCatalog {
    fn get(&self, locale: Locale, key: &str) -> Option<String> {
        self.parsed_locales()
            .get(&locale)
            .and_then(|messages| messages.get(key))
            .cloned()
            .or_else(|| {
                let fallback = self.default_locale();
                (fallback != locale)
                    .then(|| {
                        self.parsed_locales()
                            .get(&fallback)
                            .and_then(|messages| messages.get(key))
                            .cloned()
                    })
                    .flatten()
            })
    }
}

impl Catalog for StaticJsonCatalog {
    fn default_locale(&self) -> Locale {
        if self.locale_enabled(self.default_locale) {
            return self.default_locale;
        }

        self.compiled_locales()
            .into_iter()
            .next()
            .unwrap_or(self.default_locale)
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.compiled_locales()
    }
}

/// Thread-safe runtime catalog handle.
///
/// Readers access the current `Arc<dyn Catalog>` behind a shared `RwLock`.
/// Replacements swap the entire `Arc` under a write lock, so readers only see
/// fully-published catalog instances.
pub struct GlobalCatalog {
    default_locale: Locale,
    inner: RwLock<Option<Arc<dyn Catalog>>>,
}

impl GlobalCatalog {
    #[must_use]
    pub const fn new(default_locale: Locale) -> Self {
        Self {
            default_locale,
            inner: RwLock::new(None),
        }
    }

    #[must_use]
    pub fn default_locale(&self) -> Locale {
        Catalog::default_locale(self)
    }

    #[must_use]
    pub fn available_locales(&self) -> Vec<Locale> {
        Catalog::available_locales(self)
    }

    #[must_use]
    pub fn locale_enabled(&self, locale: Locale) -> bool {
        Catalog::locale_enabled(self, locale)
    }

    pub fn resolve_locale(&self, requested: Option<&str>) -> Result<Locale, String> {
        Catalog::resolve_locale(self, requested)
    }

    pub fn resolve_cli_locale(
        &self,
        args: Vec<String>,
        env_var: &str,
    ) -> Result<(Locale, Vec<String>), String> {
        Catalog::resolve_cli_locale(self, args, env_var)
    }

    pub fn replace<C>(&self, catalog: C)
    where
        C: Catalog + 'static,
    {
        self.replace_arc(Arc::new(catalog));
    }

    pub fn replace_arc(&self, catalog: Arc<dyn Catalog>) {
        *write_unpoisoned(&self.inner) = Some(catalog);
    }

    #[must_use]
    pub fn is_initialized(&self) -> bool {
        read_unpoisoned(&self.inner).is_some()
    }

    fn with_catalog<T>(&self, f: impl FnOnce(&dyn Catalog) -> T) -> Option<T> {
        read_unpoisoned(&self.inner)
            .as_ref()
            .map(|catalog| f(catalog.as_ref()))
    }
}

impl MessageCatalog for GlobalCatalog {
    fn get(&self, locale: Locale, key: &str) -> Option<String> {
        self.with_catalog(|catalog| catalog.get(locale, key))
            .flatten()
    }
}

impl Catalog for GlobalCatalog {
    fn default_locale(&self) -> Locale {
        self.with_catalog(|catalog| catalog.default_locale())
            .unwrap_or(self.default_locale)
    }

    fn available_locales(&self) -> Vec<Locale> {
        self.with_catalog(|catalog| catalog.available_locales())
            .unwrap_or_else(|| vec![self.default_locale])
    }
}

#[must_use]
pub fn render_message<C>(catalog: &C, locale: Locale, key: &str, args: &[MessageArg<'_>]) -> String
where
    C: MessageCatalog + ?Sized,
{
    let template = catalog.get(locale, key).unwrap_or_else(|| key.to_string());
    interpolate(&template, args)
}

#[must_use]
pub fn interpolate(template: &str, args: &[MessageArg<'_>]) -> String {
    let mut rendered = template.to_string();
    for arg in args {
        rendered = rendered.replace(&format!("{{{}}}", arg.name()), arg.value());
    }
    rendered
}

#[macro_export]
macro_rules! static_json_catalog {
    (
        default: $default_locale:expr,
        $($locale:expr => {
            enabled: $enabled:expr,
            json: $json:expr
        }),+ $(,)?
    ) => {{
        const SOURCES: &[$crate::StaticJsonLocale] = &[
            $(
                $crate::StaticJsonLocale::new($locale, $enabled, $json),
            )+
        ];
        $crate::StaticJsonCatalog::new($default_locale, SOURCES)
    }};
}

fn strip_locale_args<C>(
    catalog: &C,
    args: Vec<String>,
) -> Result<(Option<String>, Vec<String>), String>
where
    C: Catalog + ?Sized,
{
    let mut requested_locale: Option<String> = None;
    let mut cleaned = Vec::<String>::with_capacity(args.len());
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--lang" || arg == "--locale" {
            let value = iter.next().ok_or_else(|| {
                render_message(
                    catalog,
                    catalog.default_locale(),
                    "cli.missing_value",
                    &[MessageArg::new("flag", "--lang/--locale")],
                )
            })?;
            requested_locale = Some(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--lang=") {
            requested_locale = Some(value.to_string());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--locale=") {
            requested_locale = Some(value.to_string());
            continue;
        }
        cleaned.push(arg);
    }
    Ok((requested_locale, cleaned))
}

fn normalize_locale_id(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .split('.')
        .next()
        .unwrap_or_default()
        .split('@')
        .next()
        .unwrap_or_default()
        .trim();
    if trimmed.is_empty() {
        return None;
    }

    let raw_parts = trimmed
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if raw_parts.is_empty() {
        return None;
    }

    let mut normalized = String::new();
    for (index, part) in raw_parts.iter().enumerate() {
        if index > 0 {
            normalized.push('_');
        }

        let cleaned = match part.to_ascii_lowercase().as_str() {
            "jp" if index == 0 => "ja".to_string(),
            other if index == 0 => other.to_string(),
            _ if part.len() == 4 && part.chars().all(|ch| ch.is_ascii_alphabetic()) => {
                let mut out = String::new();
                for (char_index, ch) in part.chars().enumerate() {
                    if char_index == 0 {
                        out.push(ch.to_ascii_uppercase());
                    } else {
                        out.push(ch.to_ascii_lowercase());
                    }
                }
                out
            }
            _ => part.to_ascii_uppercase(),
        };
        normalized.push_str(&cleaned);
    }

    Some(normalized)
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poison| poison.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[derive(Debug)]
    struct TestCatalog {
        by_locale: BTreeMap<Locale, BTreeMap<&'static str, &'static str>>,
        default_locale: Locale,
    }

    impl MessageCatalog for TestCatalog {
        fn get(&self, locale: Locale, key: &str) -> Option<String> {
            self.by_locale
                .get(&locale)
                .and_then(|messages| messages.get(key).copied())
                .map(str::to_string)
        }
    }

    impl Catalog for TestCatalog {
        fn default_locale(&self) -> Locale {
            self.default_locale
        }

        fn available_locales(&self) -> Vec<Locale> {
            self.by_locale.keys().copied().collect()
        }
    }

    #[test]
    fn renders_template_interpolation() {
        let mut by_locale = BTreeMap::new();
        by_locale.insert(Locale::EnUs, BTreeMap::from([("greeting", "hello {name}")]));
        by_locale.insert(Locale::ZhCn, BTreeMap::from([("greeting", "你好，{name}")]));

        let catalog = TestCatalog {
            by_locale,
            default_locale: Locale::EnUs,
        };

        assert_eq!(
            render_message(
                &catalog,
                Locale::ZhCn,
                "greeting",
                &[MessageArg::new("name", "Alice")],
            ),
            "你好，Alice"
        );
    }

    #[test]
    fn locale_parse_accepts_open_locales() {
        assert_eq!(Locale::parse("en"), Some(Locale::EnUs));
        assert_eq!(Locale::parse("fr-FR").map(Locale::as_str), Some("fr_FR"));
        assert_eq!(
            Locale::parse("zh-Hant-TW").map(Locale::as_str),
            Some("zh_Hant_TW")
        );
    }

    #[test]
    fn static_json_catalog_falls_back_to_default_locale() {
        static SOURCES: [StaticJsonLocale; 2] = [
            StaticJsonLocale::new(Locale::EnUs, true, r#"{"greeting":"hello {name}"}"#),
            StaticJsonLocale::new(Locale::ZhCn, false, r#"{"greeting":"你好，{name}"}"#),
        ];
        static CATALOG: StaticJsonCatalog = StaticJsonCatalog::new(Locale::EnUs, &SOURCES);

        assert_eq!(CATALOG.default_locale(), Locale::EnUs);
        assert_eq!(CATALOG.available_locales(), vec![Locale::EnUs]);
        assert_eq!(
            render_message(
                &CATALOG,
                Locale::JaJp,
                "greeting",
                &[MessageArg::new("name", "Alice")],
            ),
            "hello Alice"
        );
    }

    #[test]
    fn resolve_cli_locale_strips_flags() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EnUs,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: StaticJsonCatalog = StaticJsonCatalog::new(Locale::EnUs, &SOURCES);

        let (locale, cleaned) = CATALOG
            .resolve_cli_locale(
                vec![
                    "--locale".to_string(),
                    "en_US".to_string(),
                    "--flag".to_string(),
                ],
                "DITTO_LOCALE",
            )
            .expect("resolve locale");

        assert_eq!(locale, Locale::EnUs);
        assert_eq!(cleaned, vec!["--flag".to_string()]);
    }

    #[test]
    fn global_catalog_uses_installed_catalog() {
        assert_send_sync::<GlobalCatalog>();

        static GLOBAL: GlobalCatalog = GlobalCatalog::new(Locale::EnUs);
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EnUs,
            true,
            r#"{"hello":"hello"}"#,
        )];
        let catalog = StaticJsonCatalog::new(Locale::EnUs, &SOURCES);

        GLOBAL.replace(catalog);
        assert_eq!(GLOBAL.get(Locale::EnUs, "hello"), Some("hello".to_string()));
    }
}
