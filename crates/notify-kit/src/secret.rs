//! notify-kit local secret boundary.
//!
//! Built-in sink configs accept this wrapper so notify-kit does not expose
//! `secret-kit`'s storage model as part of its own public API.

#[derive(Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct NotifySecret(String);

impl NotifySecret {
    #[must_use]
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for NotifySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NotifySecret(<redacted>)")
    }
}

impl From<String> for NotifySecret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for NotifySecret {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<&String> for NotifySecret {
    fn from(value: &String) -> Self {
        Self::new(value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::NotifySecret;

    #[test]
    fn debug_redacts_secret_contents() {
        let secret = NotifySecret::new("top-secret");
        let debug = format!("{secret:?}");
        assert!(!debug.contains("top-secret"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }
}
