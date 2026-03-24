use std::fmt::{self, Display, Formatter};

use super::catalog::Catalog;
use super::locale::{
    Locale, is_posix_default_locale_request, locale_resolution_candidates,
    normalize_locale_request, normalize_system_locale_request,
};
use super::translation::{TemplateArg, TranslationCatalog, interpolate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveLocaleError {
    UnknownLocale {
        requested: String,
    },
    LocaleNotEnabled {
        requested: String,
        available: Vec<Locale>,
    },
    MissingCliLocaleValue {
        flag: &'static str,
    },
    DuplicateCliLocaleFlag {
        flag: &'static str,
    },
    MisplacedCliLocaleFlag {
        flag: &'static str,
    },
    CatalogNotInitialized,
}

impl ResolveLocaleError {
    #[must_use]
    pub fn render<C>(&self, catalog: &C, locale: Locale) -> String
    where
        C: TranslationCatalog + ?Sized,
    {
        match self {
            Self::UnknownLocale { requested } => render_error_text(
                catalog,
                locale,
                "locale.unknown",
                &[TemplateArg::new("requested", requested.as_str())],
                self,
            ),
            Self::LocaleNotEnabled {
                requested,
                available,
            } if available.is_empty() => render_error_text(
                catalog,
                locale,
                "locale.not_enabled.none",
                &[TemplateArg::new("requested", requested.as_str())],
                self,
            ),
            Self::LocaleNotEnabled {
                requested,
                available,
            } => render_error_text(
                catalog,
                locale,
                "locale.not_enabled.available",
                &[
                    TemplateArg::new("requested", requested.as_str()),
                    TemplateArg::new("available", format_locales(available)),
                ],
                self,
            ),
            Self::MissingCliLocaleValue { flag } => render_error_text(
                catalog,
                locale,
                "cli.missing_value",
                &[TemplateArg::new("flag", *flag)],
                self,
            ),
            Self::DuplicateCliLocaleFlag { flag } => render_error_text(
                catalog,
                locale,
                "cli.duplicate_flag",
                &[TemplateArg::new("flag", *flag)],
                self,
            ),
            Self::MisplacedCliLocaleFlag { flag } => render_error_text(
                catalog,
                locale,
                "cli.misplaced_flag",
                &[TemplateArg::new("flag", *flag)],
                self,
            ),
            Self::CatalogNotInitialized => self.to_string(),
        }
    }
}

impl Display for ResolveLocaleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownLocale { requested } => {
                write!(f, "unknown locale identifier: {requested}")
            }
            Self::LocaleNotEnabled {
                requested,
                available,
            } if available.is_empty() => {
                write!(
                    f,
                    "locale is not enabled: {requested}; no locales are available"
                )
            }
            Self::LocaleNotEnabled {
                requested,
                available,
            } => write!(
                f,
                "locale is not enabled: {requested}; available locales: {}",
                format_locales(available)
            ),
            Self::MissingCliLocaleValue { flag } => {
                write!(f, "missing value for locale flag {flag}")
            }
            Self::DuplicateCliLocaleFlag { flag } => {
                write!(f, "locale flag specified multiple times: {flag}")
            }
            Self::MisplacedCliLocaleFlag { flag } => {
                write!(
                    f,
                    "locale flag must appear before positional arguments: {flag}"
                )
            }
            Self::CatalogNotInitialized => f.write_str("catalog not initialized"),
        }
    }
}

impl std::error::Error for ResolveLocaleError {}

fn format_locales(locales: &[Locale]) -> String {
    let mut rendered = String::new();

    for (index, locale) in locales.iter().enumerate() {
        if index != 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(locale.as_str());
    }

    rendered
}

/// Resolves locale from CLI arguments that do not include `argv[0]`.
///
/// Locale flags must appear before the first positional argument in `args`.
pub fn resolve_locale_from_cli_args<C>(
    catalog: &C,
    args: Vec<String>,
    env_var: &str,
) -> Result<(Locale, Vec<String>), ResolveLocaleError>
where
    C: Catalog + ?Sized,
{
    let (requested_locale, args) = strip_locale_args(args)?;
    let env_locale = std::env::var(env_var).ok();
    let locale = match select_locale_request(requested_locale.as_deref(), env_locale.as_deref()) {
        Some(LocaleRequest::Explicit(requested)) => catalog.try_resolve_locale(Some(requested))?,
        Some(LocaleRequest::Environment(requested)) => {
            catalog.try_resolve_environment_locale(requested)?
        }
        None => catalog.try_resolve_locale(None)?,
    };
    Ok((locale, args))
}

/// Resolves locale from an argv-style vector whose first element, when present,
/// is always treated as the program name.
pub fn resolve_locale_from_argv<C>(
    catalog: &C,
    argv: Vec<String>,
    env_var: &str,
) -> Result<(Locale, Vec<String>), ResolveLocaleError>
where
    C: Catalog + ?Sized,
{
    let (program, args) = split_program_from_argv(argv);
    let (locale, args) = resolve_locale_from_cli_args(catalog, args, env_var)?;
    let mut cleaned = Vec::with_capacity(args.len() + usize::from(program.is_some()));
    if let Some(program) = program {
        cleaned.push(program);
    }
    cleaned.extend(args);
    Ok((locale, cleaned))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocaleRequest<'a> {
    Explicit(&'a str),
    Environment(&'a str),
}

#[derive(Debug, Clone)]
struct CatalogLocaleSnapshot {
    default_locale: Locale,
    available_locales: Vec<Locale>,
}

impl CatalogLocaleSnapshot {
    fn capture<C>(catalog: &C) -> Self
    where
        C: Catalog + ?Sized,
    {
        Self {
            default_locale: catalog.default_locale(),
            available_locales: catalog.available_locales(),
        }
    }

    fn locale_enabled(&self, locale: Locale) -> bool {
        self.available_locales.contains(&locale)
    }

    fn resolve_request(
        &self,
        request: Option<LocaleRequest<'_>>,
    ) -> Result<Locale, ResolveLocaleError> {
        match request {
            Some(LocaleRequest::Explicit(requested)) => self.resolve_explicit(requested),
            Some(LocaleRequest::Environment(requested)) => self.resolve_environment(requested),
            None => self.resolve_default(),
        }
    }

    fn resolve_explicit(&self, requested: &str) -> Result<Locale, ResolveLocaleError> {
        let parts = normalize_locale_request(requested).ok_or_else(|| {
            ResolveLocaleError::UnknownLocale {
                requested: requested.to_string(),
            }
        })?;
        let candidates = locale_resolution_candidates(&parts);
        for locale in candidates {
            if self.locale_enabled(locale) {
                return Ok(locale);
            }
        }

        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: requested.to_string(),
            available: self.available_locales.clone(),
        })
    }

    fn resolve_environment(&self, requested: &str) -> Result<Locale, ResolveLocaleError> {
        let Some(parts) = normalize_system_locale_request(requested) else {
            return self.resolve_default();
        };

        let candidates = locale_resolution_candidates(&parts);
        for locale in candidates {
            if self.locale_enabled(locale) {
                return Ok(locale);
            }
        }

        self.resolve_default()
    }

    fn resolve_default(&self) -> Result<Locale, ResolveLocaleError> {
        if self.locale_enabled(self.default_locale) {
            return Ok(self.default_locale);
        }

        Err(ResolveLocaleError::LocaleNotEnabled {
            requested: self.default_locale.to_string(),
            available: self.available_locales.clone(),
        })
    }
}

pub(crate) fn resolve_locale_request<C>(
    catalog: &C,
    request: Option<LocaleRequest<'_>>,
) -> Result<Locale, ResolveLocaleError>
where
    C: Catalog + ?Sized,
{
    CatalogLocaleSnapshot::capture(catalog).resolve_request(request)
}

#[cfg(test)]
pub(crate) fn resolve_environment_locale_request<C>(
    catalog: &C,
    requested: &str,
) -> Result<Locale, ResolveLocaleError>
where
    C: Catalog + ?Sized,
{
    CatalogLocaleSnapshot::capture(catalog).resolve_environment(requested)
}

pub(crate) fn select_locale_request<'a>(
    requested_locale: Option<&'a str>,
    env_locale: Option<&'a str>,
) -> Option<LocaleRequest<'a>> {
    requested_locale.map(LocaleRequest::Explicit).or_else(|| {
        env_locale
            .filter(|value| !is_posix_default_locale_request(value))
            .map(LocaleRequest::Environment)
    })
}

fn strip_locale_args(
    args: Vec<String>,
) -> Result<(Option<String>, Vec<String>), ResolveLocaleError> {
    const CLI_LOCALE_FLAG: &str = "--lang/--locale";

    let mut requested_locale: Option<String> = None;
    let mut cleaned = Vec::<String>::with_capacity(args.len());
    let mut allow_locale_flag = true;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        if arg == "--" {
            cleaned.push(arg);
            cleaned.extend(args);
            break;
        }

        if arg == "--lang" || arg == "--locale" {
            if !allow_locale_flag {
                return Err(ResolveLocaleError::MisplacedCliLocaleFlag {
                    flag: CLI_LOCALE_FLAG,
                });
            }
            let Some(value) = args.next() else {
                return Err(ResolveLocaleError::MissingCliLocaleValue {
                    flag: CLI_LOCALE_FLAG,
                });
            };
            if is_missing_locale_arg_value(&value) {
                return Err(ResolveLocaleError::MissingCliLocaleValue {
                    flag: CLI_LOCALE_FLAG,
                });
            }
            set_requested_cli_locale(&mut requested_locale, value)?;
            continue;
        }

        if let Some(value) = arg
            .strip_prefix("--lang=")
            .or_else(|| arg.strip_prefix("--locale="))
        {
            if !allow_locale_flag {
                return Err(ResolveLocaleError::MisplacedCliLocaleFlag {
                    flag: CLI_LOCALE_FLAG,
                });
            }
            if is_missing_locale_arg_value(value) {
                return Err(ResolveLocaleError::MissingCliLocaleValue {
                    flag: CLI_LOCALE_FLAG,
                });
            }
            set_requested_cli_locale(&mut requested_locale, value.to_string())?;
            continue;
        }

        if !arg.starts_with('-') {
            allow_locale_flag = false;
        }

        cleaned.push(arg);
    }
    Ok((requested_locale, cleaned))
}

fn split_program_from_argv(argv: Vec<String>) -> (Option<String>, Vec<String>) {
    let mut argv = argv.into_iter();
    let Some(program) = argv.next() else {
        return (None, Vec::new());
    };

    (Some(program), argv.collect())
}

fn set_requested_cli_locale(
    requested_locale: &mut Option<String>,
    value: String,
) -> Result<(), ResolveLocaleError> {
    if requested_locale.replace(value).is_some() {
        return Err(ResolveLocaleError::DuplicateCliLocaleFlag {
            flag: "--lang/--locale",
        });
    }

    Ok(())
}

fn is_missing_locale_arg_value(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.starts_with('-')
}

fn render_error_text<C>(
    catalog: &C,
    locale: Locale,
    key: &str,
    args: &[TemplateArg<'_>],
    fallback: &ResolveLocaleError,
) -> String
where
    C: TranslationCatalog + ?Sized,
{
    catalog
        .get_template_shared(locale, key)
        .map(|template| interpolate(template.as_ref(), args))
        .unwrap_or_else(|| fallback.to_string())
}
