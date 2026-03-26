use i18n_kit::{Catalog, Locale};

use crate::catalog_error::CliLocaleError;

const CLI_LOCALE_FLAG: &str = "--lang/--locale";

/// Resolves locale from CLI arguments that do not include `argv[0]`.
///
/// Locale flags must appear before the first positional argument in `args`.
pub fn resolve_locale_from_cli_args<C>(
    catalog: &C,
    args: Vec<String>,
    env_var: &str,
) -> Result<(Locale, Vec<String>), CliLocaleError>
where
    C: Catalog + ?Sized,
{
    let (requested_locale, args) = strip_locale_args(args)?;
    let locale = match requested_locale {
        Some(requested) => catalog
            .try_resolve_locale(Some(requested.as_str()))
            .map_err(CliLocaleError::from)?,
        None => match std::env::var(env_var).ok() {
            Some(requested) => catalog
                .try_resolve_environment_locale(requested.as_str())
                .map_err(CliLocaleError::from)?,
            None => catalog
                .try_resolve_locale(None)
                .map_err(CliLocaleError::from)?,
        },
    };

    Ok((locale, args))
}

/// Resolves locale from an argv-style vector whose first element, when present,
/// is always treated as the program name.
pub fn resolve_locale_from_argv<C>(
    catalog: &C,
    argv: Vec<String>,
    env_var: &str,
) -> Result<(Locale, Vec<String>), CliLocaleError>
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

fn strip_locale_args(args: Vec<String>) -> Result<(Option<String>, Vec<String>), CliLocaleError> {
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
                return Err(CliLocaleError::MisplacedFlag {
                    flag: CLI_LOCALE_FLAG,
                });
            }
            let Some(value) = args.next() else {
                return Err(CliLocaleError::MissingValue {
                    flag: CLI_LOCALE_FLAG,
                });
            };
            if is_missing_locale_arg_value(&value) {
                return Err(CliLocaleError::MissingValue {
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
                return Err(CliLocaleError::MisplacedFlag {
                    flag: CLI_LOCALE_FLAG,
                });
            }
            if is_missing_locale_arg_value(value) {
                return Err(CliLocaleError::MissingValue {
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
) -> Result<(), CliLocaleError> {
    if requested_locale.replace(value).is_some() {
        return Err(CliLocaleError::DuplicateFlag {
            flag: CLI_LOCALE_FLAG,
        });
    }

    Ok(())
}

fn is_missing_locale_arg_value(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use i18n_kit::{Locale, StaticJsonCatalog, StaticJsonLocale};
    use std::sync::LazyLock;

    fn render_cli_locale_error<C>(catalog: &C, error: CliLocaleError) -> String
    where
        C: Catalog + ?Sized,
    {
        error.render(catalog, catalog.default_locale())
    }

    #[test]
    fn resolve_cli_locale_strips_flags() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let (locale, cleaned) = resolve_locale_from_cli_args(
            &*CATALOG,
            vec![
                "--locale".to_string(),
                "en_US".to_string(),
                "--flag".to_string(),
            ],
            "APP_LOCALE",
        )
        .expect("resolve locale");

        assert_eq!(locale, Locale::EN_US);
        assert_eq!(cleaned, vec!["--flag".to_string()]);
    }

    #[test]
    fn resolve_cli_locale_strips_top_level_flag_after_program_name() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let (locale, cleaned) = resolve_locale_from_argv(
            &*CATALOG,
            vec![
                "cmd".to_string(),
                "--locale".to_string(),
                "en_US".to_string(),
                "--flag".to_string(),
            ],
            "APP_LOCALE",
        )
        .expect("resolve locale");

        assert_eq!(locale, Locale::EN_US);
        assert_eq!(cleaned, vec!["cmd".to_string(), "--flag".to_string()]);
    }

    #[test]
    fn resolve_cli_locale_treats_flag_like_argv0_as_program_name() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(Locale::EN_US, true, "{}")];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let (locale, cleaned) = resolve_locale_from_argv(
            &*CATALOG,
            vec!["--locale".to_string(), "--flag".to_string()],
            "APP_LOCALE",
        )
        .expect("argv[0] should be treated as program name");

        assert_eq!(locale, Locale::EN_US);
        assert_eq!(cleaned, vec!["--locale".to_string(), "--flag".to_string()]);
    }

    #[test]
    fn resolve_cli_locale_rejects_flag_as_missing_value() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let error = resolve_locale_from_cli_args(
            &*CATALOG,
            vec!["--locale".to_string(), "--verbose".to_string()],
            "APP_LOCALE",
        )
        .expect_err("missing locale value");

        assert_eq!(
            render_cli_locale_error(&*CATALOG, error),
            "missing --lang/--locale"
        );
    }

    #[test]
    fn resolve_cli_locale_rejects_duplicate_locale_flags() {
        static SOURCES: [StaticJsonLocale; 2] = [
            StaticJsonLocale::new(Locale::EN_US, true, "{}"),
            StaticJsonLocale::new(Locale::from_static("fr_FR"), true, "{}"),
        ];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let error = resolve_locale_from_argv(
            &*CATALOG,
            vec![
                "cmd".to_string(),
                "--locale=en_US".to_string(),
                "--locale".to_string(),
                "fr_FR".to_string(),
            ],
            "APP_LOCALE",
        )
        .expect_err("duplicate locale flags should fail");

        assert_eq!(
            render_cli_locale_error(&*CATALOG, error),
            "locale flag specified multiple times: --lang/--locale"
        );
    }

    #[test]
    fn resolve_cli_locale_rejects_empty_equals_value() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let error =
            resolve_locale_from_cli_args(&*CATALOG, vec!["--locale=".to_string()], "APP_LOCALE")
                .expect_err("empty locale value should fail");

        assert_eq!(
            render_cli_locale_error(&*CATALOG, error),
            "missing --lang/--locale"
        );
    }

    #[test]
    fn resolve_cli_locale_rejects_empty_separate_value() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let error = resolve_locale_from_cli_args(
            &*CATALOG,
            vec!["--locale".to_string(), String::new()],
            "APP_LOCALE",
        )
        .expect_err("empty locale value should fail");

        assert_eq!(
            render_cli_locale_error(&*CATALOG, error),
            "missing --lang/--locale"
        );
    }

    #[test]
    fn resolve_cli_locale_stops_parsing_after_double_dash() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::EN_US,
            true,
            r#"{"cli.missing_value":"missing {flag}"}"#,
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::EN_US, &SOURCES).expect("valid catalog")
        });

        let (locale, cleaned) = resolve_locale_from_argv(
            &*CATALOG,
            vec![
                "cmd".to_string(),
                "--".to_string(),
                "--locale".to_string(),
                "fr_FR".to_string(),
            ],
            "APP_LOCALE",
        )
        .expect("resolve locale");

        assert_eq!(locale, Locale::EN_US);
        assert_eq!(
            cleaned,
            vec![
                "cmd".to_string(),
                "--".to_string(),
                "--locale".to_string(),
                "fr_FR".to_string()
            ]
        );
    }

    #[test]
    fn resolve_cli_locale_rejects_flag_after_first_positional_after_program_name() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::from_static("ca_ES"),
            true,
            "{}",
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::from_static("ca_ES"), &SOURCES)
                .expect("valid catalog")
        });

        let error = resolve_locale_from_argv(
            &*CATALOG,
            vec![
                "cmd".to_string(),
                "sub".to_string(),
                "--locale=ca_ES".to_string(),
            ],
            "APP_LOCALE",
        )
        .expect_err("locale flag after first positional should fail");

        assert_eq!(
            render_cli_locale_error(&*CATALOG, error),
            "locale flag must appear before positional arguments: --lang/--locale"
        );
    }

    #[test]
    fn resolve_cli_locale_rejects_env_style_explicit_locale() {
        static SOURCES: [StaticJsonLocale; 1] = [StaticJsonLocale::new(
            Locale::from_static("ca_ES"),
            true,
            "{}",
        )];
        static CATALOG: LazyLock<StaticJsonCatalog> = LazyLock::new(|| {
            StaticJsonCatalog::try_new(Locale::from_static("ca_ES"), &SOURCES)
                .expect("valid catalog")
        });

        let error = resolve_locale_from_argv(
            &*CATALOG,
            vec!["cmd".to_string(), "--locale=ca_ES@valencia".to_string()],
            "APP_LOCALE",
        )
        .expect_err("explicit locale flags should reject env-style syntax");

        assert_eq!(
            render_cli_locale_error(&*CATALOG, error),
            "unknown locale identifier: ca_ES@valencia"
        );
    }
}
