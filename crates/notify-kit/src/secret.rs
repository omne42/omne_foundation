#[derive(Clone, Default)]
pub struct SecretString(secret_kit::SecretString);

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(secret_kit::SecretString::new(value))
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }

    pub fn into_inner(self) -> std::result::Result<String, Self> {
        match self.0.into_inner() {
            Ok(value) => Ok(value),
            Err(secret) => Err(Self(secret)),
        }
    }

    #[must_use]
    pub fn into_owned(self) -> String {
        self.0.into_owned()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<secret_kit::SecretString> for SecretString {
    fn from(value: secret_kit::SecretString) -> Self {
        Self(value)
    }
}

impl From<SecretString> for secret_kit::SecretString {
    fn from(value: SecretString) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::SecretString;

    #[test]
    fn debug_redacts_secret() {
        let secret = SecretString::new("top-secret");
        let debug = format!("{secret:?}");
        assert!(!debug.contains("top-secret"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }

    #[test]
    fn converts_from_inner_secret_string() {
        let secret = SecretString::from(secret_kit::SecretString::new("token"));
        assert_eq!(secret.expose_secret(), "token");
    }
}
