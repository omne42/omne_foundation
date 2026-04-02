use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

use crate::{SecretString, command, os_env_var_name_matches};

pub trait SecretEnvironment: Send + Sync {
    fn get_secret(&self, key: &str) -> Option<SecretString>;

    /// Stable partition key used by [`crate::CachingSecretResolver`] to isolate cached secrets.
    ///
    /// The value should be stable for the lifetime of the environment instance and should reflect
    /// the non-secret context that can affect secret resolution, such as a deployment name,
    /// account identifier, or configuration profile.
    /// It must never contain secret material or request-unique noise. A partition derived from a
    /// secret value defeats the point of cache isolation, and a partition that changes on every
    /// request silently disables reuse.
    /// Different resolution contexts must return different partitions. Reusing a partition means
    /// the caller is asserting that cacheable secrets resolve identically across those instances.
    ///
    /// Returning `None` disables cache reuse for this environment. Empty partitions are treated
    /// the same way. This is the safe default when no stable, non-secret partition key exists.
    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        None
    }
}

/// Command-execution policy used by CLI-backed secret providers.
///
/// This is intentionally separate from [`SecretEnvironment`]. Secret lookup is domain state;
/// child-process environment shaping and binary resolution are runtime policy.
///
/// Async secret resolution for CLI-backed providers enforces command timeouts via `tokio::time`,
/// so callers need a Tokio runtime with the time driver enabled.
pub trait SecretCommandRuntime: Send + Sync {
    /// Stable partition key used by [`crate::CachingSecretResolver`] for runtime-sensitive secrets.
    ///
    /// Cacheable resolutions that depend on command discovery, explicit command environment, or
    /// other runtime policy should include this partition in their cache key. Returning `None`
    /// disables cache reuse for runtime-sensitive secrets, which is the safe default when no
    /// stable, non-secret runtime identity exists. The built-in ambient runtime intentionally
    /// returns `None` here because the process environment and `PATH` are not a stable cache
    /// boundary.
    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Targeted command-environment lookup for control-plane settings and runtime overrides.
    ///
    /// This does not automatically populate spawned child processes. Use `command_env_pairs` or
    /// `command_env_os_pairs` for explicit child environment injection.
    /// Secret command timeout tuning also does not consult this hook; if you want a resolver-local
    /// timeout, put the `SECRET_COMMAND_TIMEOUT_*` variables into the explicit command
    /// snapshot instead of relying on ambient process state.
    fn get_command_env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    /// Targeted command-environment lookup for control-plane settings and runtime overrides.
    ///
    /// Look up a command-environment value while sharing the same explicit snapshot used for child
    /// process injection when possible.
    fn get_command_env_os(&self, key: &OsStr) -> Option<OsString> {
        self.command_env_os_pairs()
            .find_map(|(candidate, value)| {
                os_env_var_name_matches(candidate.as_os_str(), key).then_some(value)
            })
            .or_else(|| {
                key.to_str()
                    .and_then(|key| self.get_command_env(key).map(OsString::from))
            })
    }

    /// Explicit child-process environment snapshot.
    ///
    /// Values returned here are injected into spawned commands after the ambient allowlist.
    fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
        Box::new(std::iter::empty())
    }

    fn command_env_os_pairs(&self) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
        Box::new(
            self.command_env_pairs()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
    }

    fn ambient_command_env_pairs(
        &self,
        program: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + '_> {
        command::filtered_ambient_command_env_pairs(program)
    }

    fn ambient_command_env_os_pairs(
        &self,
        program: &str,
    ) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
        command::filtered_ambient_command_env_os_pairs(program)
    }

    /// Resolve the executable used for a secret CLI command.
    ///
    /// Built-in providers only accept absolute override paths whose basename still matches the
    /// original provider binary (for example `/tmp/vault` for `vault`). Without an override they
    /// resolve the program from trusted system directories in the ambient allowlisted `PATH`
    /// snapshot, not from arbitrary absolute search entries or explicit `command_env_pairs`
    /// injection.
    fn resolve_command_program(&self, _program: &str) -> Option<String> {
        None
    }
}

#[derive(Clone, Copy)]
pub struct SecretResolutionContext<'a> {
    environment: &'a dyn SecretEnvironment,
    command_runtime: &'a dyn SecretCommandRuntime,
}

impl<'a> SecretResolutionContext<'a> {
    #[must_use]
    pub fn new(
        environment: &'a dyn SecretEnvironment,
        command_runtime: &'a dyn SecretCommandRuntime,
    ) -> Self {
        Self {
            environment,
            command_runtime,
        }
    }

    #[must_use]
    pub fn ambient(environment: &'a dyn SecretEnvironment) -> Self {
        Self::new(environment, &AMBIENT_SECRET_COMMAND_RUNTIME)
    }

    #[must_use]
    pub fn environment(self) -> &'a dyn SecretEnvironment {
        self.environment
    }

    #[must_use]
    pub fn command_runtime(self) -> &'a dyn SecretCommandRuntime {
        self.command_runtime
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AmbientSecretCommandRuntime;

impl SecretCommandRuntime for AmbientSecretCommandRuntime {}

static AMBIENT_SECRET_COMMAND_RUNTIME: AmbientSecretCommandRuntime = AmbientSecretCommandRuntime;
