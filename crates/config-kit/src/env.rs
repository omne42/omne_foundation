use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvInterpolationOptions {
    max_output_bytes: usize,
}

impl EnvInterpolationOptions {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_output_bytes: super::load::DEFAULT_MAX_CONFIG_BYTES as usize,
        }
    }

    #[must_use]
    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    #[must_use]
    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

impl Default for EnvInterpolationOptions {
    fn default() -> Self {
        Self::new()
    }
}

pub fn interpolate_env_placeholders(raw: &str) -> Result<String> {
    interpolate_env_placeholders_with(raw, EnvInterpolationOptions::default(), |name| {
        std::env::var(name).ok()
    })
}

pub fn interpolate_env_placeholders_with<F>(
    raw: &str,
    options: EnvInterpolationOptions,
    mut lookup: F,
) -> Result<String>
where
    F: FnMut(&str) -> Option<String>,
{
    if options.max_output_bytes == 0 {
        return Err(Error::EnvInterpolation {
            message: "max_output_bytes must be greater than zero".to_string(),
        });
    }

    let bytes = raw.as_bytes();
    let mut output = String::with_capacity(raw.len());
    let mut idx = 0usize;
    let mut last = 0usize;

    while idx + 1 < bytes.len() {
        if bytes[idx] == b'$' && bytes[idx + 1] == b'{' {
            output.push_str(&raw[last..idx]);
            ensure_output_limit(&output, options.max_output_bytes)?;

            let start = idx + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'}' {
                end += 1;
            }
            if end >= bytes.len() {
                return Err(Error::EnvInterpolation {
                    message: "unterminated ${...} placeholder".to_string(),
                });
            }

            let name = &raw[start..end];
            if !is_valid_env_var_name(name) {
                return Err(Error::EnvInterpolation {
                    message: format!("invalid env var name {name:?}"),
                });
            }
            let value = lookup(name).ok_or_else(|| Error::EnvInterpolation {
                message: format!("env var {name:?} is not set"),
            })?;
            if output
                .len()
                .checked_add(value.len())
                .is_none_or(|len| len > options.max_output_bytes)
            {
                return Err(Error::EnvInterpolation {
                    message: format!(
                        "output exceeds max size {} bytes after expanding {name:?}",
                        options.max_output_bytes
                    ),
                });
            }
            output.push_str(&value);
            idx = end + 1;
            last = idx;
            continue;
        }
        idx += 1;
    }

    output.push_str(&raw[last..]);
    ensure_output_limit(&output, options.max_output_bytes)?;
    Ok(output)
}

#[must_use]
pub fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn ensure_output_limit(output: &str, max_output_bytes: usize) -> Result<()> {
    if output.len() <= max_output_bytes {
        return Ok(());
    }
    Err(Error::EnvInterpolation {
        message: format!("output exceeds max size {max_output_bytes} bytes"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_env_placeholders() {
        let rendered = interpolate_env_placeholders_with(
            "hello ${NAME}",
            EnvInterpolationOptions::new(),
            |name| (name == "NAME").then(|| "world".to_string()),
        )
        .expect("interpolate");
        assert_eq!(rendered, "hello world");
    }

    #[test]
    fn rejects_invalid_names() {
        let err =
            interpolate_env_placeholders_with("${1BAD}", EnvInterpolationOptions::new(), |_| None)
                .expect_err("invalid name");
        assert!(err.to_string().contains("invalid env var name"));
    }

    #[test]
    fn rejects_unterminated_placeholders() {
        let err =
            interpolate_env_placeholders_with("${NAME", EnvInterpolationOptions::new(), |_| {
                Some("world".to_string())
            })
            .expect_err("unterminated");
        assert!(err.to_string().contains("unterminated"));
    }

    #[test]
    fn rejects_missing_values() {
        let err =
            interpolate_env_placeholders_with("${NAME}", EnvInterpolationOptions::new(), |_| None)
                .expect_err("missing");
        assert!(err.to_string().contains("is not set"));
    }

    #[test]
    fn enforces_output_limit() {
        let err = interpolate_env_placeholders_with(
            "${NAME}",
            EnvInterpolationOptions::new().with_max_output_bytes(2),
            |_| Some("world".to_string()),
        )
        .expect_err("too large");
        assert!(err.to_string().contains("max size 2 bytes"));
    }
}
