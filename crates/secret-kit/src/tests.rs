use std::borrow::Cow;
use std::error::Error as _;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::*;
#[cfg(unix)]
use crate::command::trusted_builtin_search_directory_metadata_for_test;
use crate::command::{
    build_command_env, resolve_program_on_path_for_test, run_secret_command,
    sanitize_ambient_command_search_path_for_test, secret_command_timeout_from_env,
};
use crate::json::extract_json_key;
use crate::spec::{
    SecretCommand, build_secret_command, prepare_default_secret_spec_resolution,
    resolve_prepared_default_secret,
};
use error_kit::{ErrorCategory, ErrorRetryAdvice};
use structured_text_kit::{CatalogTextRef, StructuredText, structured_text};

fn assert_catalog_code(text: &StructuredText, expected: &str) {
    assert_eq!(catalog(text).code(), expected);
}

fn catalog(text: &StructuredText) -> CatalogTextRef<'_> {
    text.as_catalog()
        .expect("test text should be catalog-backed")
}

fn assert_catalog_text_arg(text: &StructuredText, name: &str, expected: Option<&str>) {
    assert_eq!(catalog(text).text_arg(name), expected);
}

fn assert_secret_error_code(error: &SecretError, expected: &str) {
    assert_eq!(error.error_code().as_str(), expected);
}

#[cfg(not(windows))]
fn assert_catalog_arg_missing(text: &StructuredText, name: &str) {
    assert_eq!(catalog(text).arg(name), None);
}

#[cfg(unix)]
fn write_executable_script(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let temp_path = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;

    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o755);
    file.set_permissions(permissions)?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(temp_path, path)
}

struct TestEnv {
    cache_partition: String,
    runtime_cache_partition: Option<String>,
    vars: BTreeMap<String, String>,
    command_vars: BTreeMap<String, String>,
    command_programs: BTreeMap<String, String>,
}

impl Default for TestEnv {
    fn default() -> Self {
        Self {
            cache_partition: "default-test-env".to_string(),
            runtime_cache_partition: Some("default-test-runtime".to_string()),
            vars: BTreeMap::new(),
            command_vars: BTreeMap::new(),
            command_programs: BTreeMap::new(),
        }
    }
}

impl SecretEnvironment for TestEnv {
    fn get_secret(&self, key: &str) -> Option<SecretString> {
        self.vars.get(key).cloned().map(SecretString::from)
    }

    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.cache_partition.as_str()))
    }
}

impl SecretCommandRuntime for TestEnv {
    fn secret_cache_partition(&self) -> Option<Cow<'_, str>> {
        self.runtime_cache_partition.as_deref().map(Cow::Borrowed)
    }

    fn get_command_env(&self, key: &str) -> Option<String> {
        self.command_vars
            .get(key)
            .or_else(|| self.vars.get(key))
            .cloned()
            .or_else(|| std::env::var(key).ok())
    }

    fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
        let mut env = std::env::vars().collect::<Vec<_>>();
        env.extend(
            self.vars
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        env.extend(
            self.command_vars
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Box::new(env.into_iter())
    }

    fn resolve_command_program(&self, program: &str) -> Option<String> {
        self.command_programs.get(program).cloned()
    }
}

async fn resolve_secret_text<E>(spec: &str, env: &E) -> Result<String>
where
    E: SecretEnvironment + SecretCommandRuntime,
{
    resolve_secret(spec, env)
        .await
        .map(|secret| secret.expose_secret().to_owned())
}

#[test]
fn detects_secret_spec_strings() {
    assert!(looks_like_secret_spec("secret://env/OPENAI_API_KEY"));
    assert!(looks_like_secret_spec("  secret://env/OPENAI_API_KEY  "));
    assert!(!looks_like_secret_spec("${OPENAI_API_KEY}"));
    assert!(!looks_like_secret_spec("https://example.com"));
}

fn test_env_spec(key: &str) -> String {
    format!("secret://env/{key}")
}

fn env_spec_key(spec: &SecretSpec) -> &str {
    match spec {
        SecretSpec::Env { key } => key,
        other => panic!("expected env secret spec, got {other:?}"),
    }
}

trait TestSecretResolverExt: SecretResolver {
    async fn resolve_secret_spec_text(
        &self,
        spec: &SecretSpec,
        env: &dyn SecretEnvironment,
    ) -> Result<String> {
        self.resolve_secret_spec(spec, SecretResolutionContext::ambient(env))
            .await
            .map(|secret| secret.expose_secret().to_owned())
    }

    async fn resolve_secret_text(&self, spec: &str, env: &dyn SecretEnvironment) -> Result<String> {
        self.resolve_secret(spec, SecretResolutionContext::ambient(env))
            .await
            .map(|secret| secret.expose_secret().to_owned())
    }

    async fn resolve_secret_text_with_runtime(
        &self,
        spec: &str,
        environment: &dyn SecretEnvironment,
        runtime: &dyn SecretCommandRuntime,
    ) -> Result<String> {
        self.resolve_secret(spec, SecretResolutionContext::new(environment, runtime))
            .await
            .map(|secret| secret.expose_secret().to_owned())
    }
}

impl<T> TestSecretResolverExt for T where T: SecretResolver + ?Sized {}

struct LegacyStringResolver;

impl SecretResolver for LegacyStringResolver {
    fn resolve_secret<'a>(
        &'a self,
        spec: &'a str,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move { resolve_secret_in_context(spec, context).await })
    }
}

struct DefaultOnlyResolver;

impl SecretResolver for DefaultOnlyResolver {}

struct DefaultOnlyCacheAwareResolver;

impl SecretResolver for DefaultOnlyCacheAwareResolver {}

impl CacheAwareSecretResolver for DefaultOnlyCacheAwareResolver {
    type Prepared = ();

    async fn resolve_prepared_secret(
        &self,
        _prepared: Self::Prepared,
        _context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        Ok(SecretString::from("unused"))
    }
}

#[tokio::test]
async fn secret_resolver_trait_object_resolves_secret() -> Result<()> {
    let mut env = TestEnv::default();
    env.vars
        .insert("OPENAI_API_KEY".to_string(), "test-secret".to_string());

    let boxed_resolver: Box<dyn SecretResolver> = Box::new(DefaultSecretResolver);
    let boxed_value = boxed_resolver
        .resolve_secret_text("secret://env/OPENAI_API_KEY", &env)
        .await?;
    assert_eq!(boxed_value, "test-secret");

    let arc_resolver: Arc<dyn SecretResolver> = Arc::new(DefaultSecretResolver);
    let arc_value = arc_resolver
        .resolve_secret_text("secret://env/OPENAI_API_KEY", &env)
        .await?;

    assert_eq!(arc_value, "test-secret");
    Ok(())
}

#[tokio::test]
async fn resolve_string_if_secret_returns_literal_for_non_secret_values() -> Result<()> {
    let env = TestEnv::default();
    let value = resolve_string_if_secret("https://example.com/v1", &env).await?;
    assert_eq!(value, "https://example.com/v1");
    Ok(())
}

#[tokio::test]
async fn resolve_string_if_secret_resolves_secret_specs() -> Result<()> {
    let mut env = TestEnv::default();
    env.vars
        .insert("OPENAI_API_KEY".to_string(), "test-secret".to_string());

    let value = resolve_string_if_secret(" secret://env/OPENAI_API_KEY ", &env).await?;
    assert_eq!(value, "test-secret");
    Ok(())
}

#[tokio::test]
async fn secret_resolver_trait_object_resolves_parsed_secret() -> Result<()> {
    let mut env = TestEnv::default();
    env.vars
        .insert("OPENAI_API_KEY".to_string(), "test-secret".to_string());
    let spec = SecretSpec::parse("secret://env/OPENAI_API_KEY")?;

    let boxed_resolver: Box<dyn SecretResolver> = Box::new(DefaultSecretResolver);
    let boxed_value = boxed_resolver.resolve_secret_spec_text(&spec, &env).await?;
    assert_eq!(boxed_value, "test-secret");

    let arc_resolver: Arc<dyn SecretResolver> = Arc::new(DefaultSecretResolver);
    let arc_value = arc_resolver.resolve_secret_spec_text(&spec, &env).await?;

    assert_eq!(arc_value, "test-secret");
    Ok(())
}

#[tokio::test]
async fn legacy_secret_resolver_still_resolves_parsed_secret() -> Result<()> {
    let mut env = TestEnv::default();
    env.vars
        .insert("OPENAI_API_KEY".to_string(), "test-secret".to_string());
    let spec = SecretSpec::parse("secret://env/OPENAI_API_KEY")?;

    let value = LegacyStringResolver
        .resolve_secret_spec_text(&spec, &env)
        .await?;

    assert_eq!(value, "test-secret");
    Ok(())
}

#[tokio::test]
async fn default_only_secret_resolver_fails_closed_instead_of_recursing() -> Result<()> {
    let env = TestEnv::default();
    let spec = SecretSpec::parse("secret://env/OPENAI_API_KEY")?;
    assert_eq!(secret_resolver_default_guard_depth_for_test(), 0);

    let err = match DefaultOnlyResolver
        .resolve_secret_spec_text(&spec, &env)
        .await
    {
        Ok(_) => panic!("default-only resolver should fail closed"),
        Err(err) => err,
    };

    assert_secret_error_code(&err, "secret.invalid_spec");
    assert_catalog_code(err.structured_text(), "error_detail.secret.not_resolvable");
    assert_eq!(secret_resolver_default_guard_depth_for_test(), 0);
    Ok(())
}

#[tokio::test]
async fn default_only_cache_prepare_fails_closed_instead_of_recursing() -> Result<()> {
    let env = TestEnv::default();
    let spec = SecretSpec::parse("secret://env/OPENAI_API_KEY")?;
    assert_eq!(cache_prepare_default_guard_depth_for_test(), 0);

    let err = match DefaultOnlyCacheAwareResolver
        .prepare_secret_spec_resolution(&spec, SecretResolutionContext::ambient(&env))
        .await
    {
        Ok(_) => panic!("default-only cache-aware resolver should fail closed"),
        Err(err) => err,
    };

    assert_secret_error_code(&err, "secret.invalid_spec");
    assert_catalog_code(err.structured_text(), "error_detail.secret.not_resolvable");
    assert_eq!(cache_prepare_default_guard_depth_for_test(), 0);
    Ok(())
}

fn test_cache_scope(spec: &SecretSpec) -> Option<String> {
    match spec {
        SecretSpec::File { path } if Path::new(path).is_absolute() => {
            let metadata = std::fs::metadata(path).ok()?;
            Some(format!("test-file:{path}:{}", metadata.len()))
        }
        _ => None,
    }
}

#[allow(dead_code)]
#[derive(Default)]
struct LegacyCountingResolver {
    calls: AtomicUsize,
}

impl SecretResolver for LegacyCountingResolver {
    fn resolve_secret<'a>(
        &'a self,
        _spec: &'a str,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(SecretString::from(format!("value-{call}")))
        })
    }
}

impl CacheAwareSecretResolver for LegacyCountingResolver {
    type Prepared = ();

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        let parsed = SecretSpec::parse(spec)?;
        Ok(match test_cache_scope(&parsed) {
            Some(scope) => PreparedSecretResolution::cached((), scope),
            None => PreparedSecretResolution::uncached(()),
        })
    }

    async fn resolve_prepared_secret(
        &self,
        _prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret("secret://env/IGNORED", context).await
    }
}

#[derive(Default)]
struct CountingResolver {
    calls: AtomicUsize,
}

impl SecretResolver for CountingResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        _spec: &'a SecretSpec,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(SecretString::from(format!("value-{call}")))
        })
    }
}

impl CacheAwareSecretResolver for CountingResolver {
    type Prepared = ();

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(match test_cache_scope(spec) {
            Some(scope) => PreparedSecretResolution::cached((), scope),
            None => PreparedSecretResolution::uncached(()),
        })
    }

    async fn resolve_prepared_secret(
        &self,
        _prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&SecretSpec::Env { key: String::new() }, context)
            .await
    }
}

#[derive(Default)]
struct RetryResolver {
    calls: AtomicUsize,
}

impl SecretResolver for RetryResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        _spec: &'a SecretSpec,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                return Err(invalid_response!("error_detail.secret.not_resolvable"));
            }
            Ok(SecretString::from("recovered"))
        })
    }
}

impl CacheAwareSecretResolver for RetryResolver {
    type Prepared = ();

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(match test_cache_scope(spec) {
            Some(scope) => PreparedSecretResolution::cached((), scope),
            None => PreparedSecretResolution::uncached(()),
        })
    }

    async fn resolve_prepared_secret(
        &self,
        _prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&SecretSpec::Env { key: String::new() }, context)
            .await
    }
}

struct SlowResolver {
    calls: AtomicUsize,
    delay: Duration,
}

impl Default for SlowResolver {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(50),
        }
    }
}

impl SecretResolver for SlowResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        _spec: &'a SecretSpec,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::time::sleep(self.delay).await;
            Ok(SecretString::from(format!("value-{call}")))
        })
    }
}

impl CacheAwareSecretResolver for SlowResolver {
    type Prepared = ();

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(match test_cache_scope(spec) {
            Some(scope) => PreparedSecretResolution::cached((), scope),
            None => PreparedSecretResolution::uncached(()),
        })
    }

    async fn resolve_prepared_secret(
        &self,
        _prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&SecretSpec::Env { key: String::new() }, context)
            .await
    }
}

#[derive(Default)]
struct MismatchedHintResolver {
    calls: AtomicUsize,
}

impl SecretResolver for MismatchedHintResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a SecretSpec,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let key = env_spec_key(spec);
            Ok(SecretString::from(format!(
                "{key}-{}",
                self.calls.fetch_add(1, Ordering::SeqCst) + 1
            )))
        })
    }
}

#[derive(Default)]
struct SlowMismatchedHintResolver {
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl SecretResolver for SlowMismatchedHintResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a SecretSpec,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let key = env_spec_key(spec).to_string();
            self.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(SecretString::from(key))
        })
    }
}

impl CacheAwareSecretResolver for SlowMismatchedHintResolver {
    type Prepared = SecretSpec;

    fn lookup_secret_cache_scope_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some("shared-hint".to_string()))
    }

    fn lookup_secret_cache_partitioning_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        Some(SecretCachePartitioning::Environment)
    }

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(PreparedSecretResolution::cached(
            spec.clone(),
            format!("prepared:{}", env_spec_key(spec)),
        ))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&prepared, context).await
    }
}

impl CacheAwareSecretResolver for MismatchedHintResolver {
    type Prepared = SecretSpec;

    fn lookup_secret_cache_scope_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some("shared-hint".to_string()))
    }

    fn lookup_secret_cache_partitioning_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        Some(SecretCachePartitioning::Environment)
    }

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(PreparedSecretResolution::cached(
            spec.clone(),
            format!("prepared:{}", env_spec_key(spec)),
        ))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&prepared, context).await
    }
}

#[derive(Default)]
struct EnvironmentScopedResolver {
    calls: AtomicUsize,
}

#[derive(Default)]
struct MixedHintResolver {
    calls: AtomicUsize,
}

impl SecretResolver for MixedHintResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a SecretSpec,
        _context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            let key = env_spec_key(spec);
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(SecretString::from(format!("{key}-{call}")))
        })
    }
}

impl CacheAwareSecretResolver for MixedHintResolver {
    type Prepared = SecretSpec;

    fn lookup_secret_cache_scope_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some("shared-hint".to_string()))
    }

    fn lookup_secret_cache_partitioning_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        Some(SecretCachePartitioning::Environment)
    }

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        let key = env_spec_key(spec);
        let cache_scope = if key == "SPEC_A" {
            "shared-hint".to_string()
        } else {
            format!("prepared:{key}")
        };
        Ok(PreparedSecretResolution::cached(spec.clone(), cache_scope))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&prepared, context).await
    }
}

impl SecretResolver for EnvironmentScopedResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        spec: &'a SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(context
                .environment()
                .get_secret(env_spec_key(spec))
                .expect("test env secret should exist for environment-scoped cache test"))
        })
    }
}

impl CacheAwareSecretResolver for EnvironmentScopedResolver {
    type Prepared = SecretSpec;

    fn lookup_secret_cache_scope_for_spec(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some(env_spec_key(spec).to_string()))
    }

    fn lookup_secret_cache_partitioning_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        Some(SecretCachePartitioning::Environment)
    }

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        let key = env_spec_key(spec).to_string();
        Ok(PreparedSecretResolution::cached(spec.clone(), key))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&prepared, context).await
    }
}

#[derive(Default)]
struct RuntimeScopedResolver {
    calls: AtomicUsize,
}

impl SecretResolver for RuntimeScopedResolver {
    fn resolve_secret_spec<'a>(
        &'a self,
        _spec: &'a SecretSpec,
        context: SecretResolutionContext<'a>,
    ) -> SecretResolveFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let value = context
                .command_runtime()
                .get_command_env("RUNTIME_SECRET")
                .expect("test runtime secret should exist");
            Ok(SecretString::from(value))
        })
    }
}

impl CacheAwareSecretResolver for RuntimeScopedResolver {
    type Prepared = SecretSpec;

    fn lookup_secret_cache_scope_for_spec(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some(env_spec_key(spec).to_string()))
    }

    fn lookup_secret_cache_partitioning_for_spec(
        &self,
        _spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Option<SecretCachePartitioning> {
        Some(SecretCachePartitioning::EnvironmentAndCommandRuntime)
    }

    async fn prepare_secret_spec_resolution(
        &self,
        spec: &SecretSpec,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        let key = env_spec_key(spec).to_string();
        Ok(PreparedSecretResolution::cached_with_partitioning(
            spec.clone(),
            key,
            SecretCachePartitioning::EnvironmentAndCommandRuntime,
        ))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret_spec(&prepared, context).await
    }
}

async fn temp_file_spec(name: &str, contents: &[u8]) -> Result<(tempfile::TempDir, String)> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    tokio::fs::write(&path, contents).await?;
    Ok((
        dir,
        format!("secret://file?path={}", path.to_string_lossy()),
    ))
}

#[cfg(all(unix, target_os = "linux"))]
#[derive(Clone, Copy, Debug)]
struct LinuxTestProcessIdentity {
    pid: u32,
    start_ticks: Option<u64>,
}

#[cfg(all(unix, target_os = "linux"))]
fn linux_process_start_ticks(pid: u32) -> std::io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let tail = stat
        .rsplit_once(") ")
        .map(|(_, tail)| tail)
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid /proc stat")
        })?;
    let mut fields = tail.split_whitespace();
    let _state = fields.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing proc state")
    })?;
    let _parent_pid = fields.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing proc parent pid")
    })?;
    let _process_group_id = fields.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing proc group id")
    })?;
    for _ in 0..16 {
        let _ = fields.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "missing proc stat field")
        })?;
    }
    fields
        .next()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "missing proc start time")
        })?
        .parse::<u64>()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

#[cfg(all(unix, target_os = "linux"))]
fn process_terminated_or_reused_or_zombie(identity: LinuxTestProcessIdentity) -> bool {
    let pid = identity.pid;
    let Some(expected_start_ticks) = identity.start_ticks else {
        return true;
    };
    let status_path = format!("/proc/{pid}/status");
    let status = match std::fs::read_to_string(status_path) {
        Ok(status) => status,
        Err(err) => return err.kind() == std::io::ErrorKind::NotFound,
    };
    let Ok(start_ticks) = linux_process_start_ticks(pid) else {
        return true;
    };
    if start_ticks != expected_start_ticks {
        return true;
    }
    status
        .lines()
        .find(|line| line.starts_with("State:"))
        .map(|line| line.contains("\tZ") || line.contains(" zombie"))
        .unwrap_or(false)
}

#[cfg(all(unix, target_os = "linux"))]
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(all(unix, target_os = "linux"))]
const PID_FILE_WAIT_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(all(unix, target_os = "linux"))]
const PROCESS_TERMINATION_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(all(unix, target_os = "linux"))]
async fn wait_for_pid(path: &std::path::Path) -> Option<LinuxTestProcessIdentity> {
    let deadline = tokio::time::Instant::now() + PID_FILE_WAIT_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(raw) = tokio::fs::read_to_string(path).await
            && let Ok(pid) = raw.trim().parse::<u32>()
        {
            return Some(LinuxTestProcessIdentity {
                pid,
                start_ticks: linux_process_start_ticks(pid).ok(),
            });
        }
        tokio::time::sleep(PROCESS_POLL_INTERVAL).await;
    }
    None
}

#[cfg(all(unix, target_os = "linux"))]
async fn wait_for_process_termination(
    identity: LinuxTestProcessIdentity,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if process_terminated_or_reused_or_zombie(identity) {
            return true;
        }
        tokio::time::sleep(PROCESS_POLL_INTERVAL).await;
    }
    process_terminated_or_reused_or_zombie(identity)
}

#[tokio::test]
async fn resolves_env_secret() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([("TEST_SECRET".to_string(), "ok".to_string())]),
        ..TestEnv::default()
    };
    let value = resolve_secret_text("secret://env/TEST_SECRET", &env).await?;
    assert_eq!(value, "ok");
    Ok(())
}

#[tokio::test]
async fn resolve_secret_returns_redacted_secret_type() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([("TEST_SECRET".to_string(), "top-secret".to_string())]),
        ..TestEnv::default()
    };

    let value = resolve_secret("secret://env/TEST_SECRET", &env).await?;

    assert_eq!(value.expose_secret(), "top-secret");
    assert_eq!(format!("{value:?}"), "SecretString(<redacted>)");
    Ok(())
}

#[test]
fn secret_string_into_inner_requires_unique_ownership() {
    let secret = SecretString::from("top-secret");
    let shared = secret.clone();

    let err = secret
        .into_inner()
        .expect_err("shared secret should not yield owned plaintext");

    assert_eq!(err.expose_secret(), "top-secret");
    assert_eq!(shared.expose_secret(), "top-secret");
}

#[test]
fn secret_string_into_owned_reuses_unique_buffer() {
    let secret = SecretString::from("top-secret");

    let owned = secret.into_owned();

    assert_eq!(owned, "top-secret");
}

#[test]
fn secret_string_into_owned_clones_when_shared() {
    let secret = SecretString::from("top-secret");
    let shared = secret.clone();

    let owned = secret.into_owned();

    assert_eq!(owned, "top-secret");
    assert_eq!(shared.expose_secret(), "top-secret");
}

#[test]
fn secret_bytes_debug_is_redacted() {
    let bytes = SecretBytes(vec![1, 2, 3, 4]);

    assert_eq!(format!("{bytes:?}"), "SecretBytes(<redacted>, len=4)");
}

#[test]
fn deterministic_secret_io_errors_are_not_retryable() {
    let err = SecretError::io(
        structured_text!("error_detail.secret.file_not_regular"),
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
    );

    assert_eq!(err.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    assert_eq!(
        err.error_record().retry_advice(),
        ErrorRetryAdvice::DoNotRetry
    );
}

#[test]
fn transient_secret_io_errors_remain_retryable() {
    let err = SecretError::io(
        structured_text!("error_detail.secret.file_read_failed"),
        std::io::Error::new(std::io::ErrorKind::TimedOut, "temporary timeout"),
    );

    assert_eq!(err.retry_advice(), ErrorRetryAdvice::Retryable);
    assert_eq!(
        err.error_record().retry_advice(),
        ErrorRetryAdvice::Retryable
    );
}

#[test]
fn secret_command_timeout_and_spawn_failures_are_retryable() {
    let timeout = SecretError::Command(structured_text!("error_detail.secret.command_timeout"));
    let spawn = SecretError::Command(structured_text!("error_detail.secret.command_spawn_failed"));

    assert_eq!(timeout.retry_advice(), ErrorRetryAdvice::Retryable);
    assert_eq!(spawn.retry_advice(), ErrorRetryAdvice::Retryable);
}

#[test]
fn secret_command_exit_failures_are_not_retryable() {
    let err = SecretError::Command(structured_text!("error_detail.secret.command_failed"));

    assert_eq!(err.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    assert_eq!(
        err.error_record().retry_advice(),
        ErrorRetryAdvice::DoNotRetry
    );
}

#[tokio::test]
async fn resolves_env_secret_preserves_whitespace() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([("TEST_SECRET".to_string(), "  ok \n".to_string())]),
        ..TestEnv::default()
    };
    let value = resolve_secret_text("secret://env/TEST_SECRET", &env).await?;
    assert_eq!(value, "  ok \n");
    Ok(())
}

#[tokio::test]
async fn invalid_utf8_file_secret_is_not_retryable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret.bin");
    tokio::fs::write(&path, [0xff, 0xfe])
        .await
        .expect("write invalid utf8 bytes");
    let env = TestEnv::default();
    let err = resolve_secret_text(
        &format!("secret://file?path={}", path.to_string_lossy()),
        &env,
    )
    .await
    .expect_err("invalid utf8 should fail");

    let SecretError::Io { .. } = &err else {
        panic!("expected io error");
    };
    assert_eq!(err.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    assert_eq!(
        err.error_record().retry_advice(),
        ErrorRetryAdvice::DoNotRetry
    );
}

#[tokio::test]
async fn resolves_empty_env_secret() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([("TEST_SECRET".to_string(), String::new())]),
        ..TestEnv::default()
    };
    let value = resolve_secret_text("secret://env/TEST_SECRET", &env).await?;
    assert_eq!(value, "");
    Ok(())
}

#[tokio::test]
async fn resolves_file_secret_preserves_whitespace() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret.txt");
    tokio::fs::write(&path, "  hello  \n").await?;
    let env = TestEnv::default();
    let value = resolve_secret_text(
        &format!("secret://file?path={}", path.to_string_lossy()),
        &env,
    )
    .await?;
    assert_eq!(value, "  hello  \n");
    Ok(())
}

#[tokio::test]
async fn resolves_empty_file_secret() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("empty.txt");
    tokio::fs::write(&path, "").await?;
    let env = TestEnv::default();
    let value = resolve_secret_text(
        &format!("secret://file?path={}", path.to_string_lossy()),
        &env,
    )
    .await?;
    assert_eq!(value, "");
    Ok(())
}

#[tokio::test]
async fn resolves_file_secret_at_size_limit() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("max-size.txt");
    tokio::fs::write(&path, vec![b'a'; MAX_SECRET_FILE_BYTES]).await?;
    let env = TestEnv::default();
    let value = resolve_secret_text(
        &format!("secret://file?path={}", path.to_string_lossy()),
        &env,
    )
    .await?;
    assert_eq!(value.len(), MAX_SECRET_FILE_BYTES);
    assert!(value.bytes().all(|byte| byte == b'a'));
    Ok(())
}

#[tokio::test]
async fn rejects_file_secret_larger_than_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("too-large.txt");
    tokio::fs::write(&path, vec![b'a'; MAX_SECRET_FILE_BYTES + 1])
        .await
        .expect("write test file");
    let env = TestEnv::default();
    let err = resolve_secret_text(
        &format!("secret://file?path={}", path.to_string_lossy()),
        &env,
    )
    .await
    .unwrap_err();

    let SecretError::Io { .. } = &err else {
        panic!("expected io error");
    };
    assert_catalog_code(err.structured_text(), "error_detail.secret.file_too_large");
    assert_catalog_text_arg(
        err.structured_text(),
        "path",
        Some(path.to_string_lossy().as_ref()),
    );
    assert_catalog_text_arg(
        err.structured_text(),
        "max_bytes",
        Some(MAX_SECRET_FILE_BYTES.to_string().as_str()),
    );
}

#[tokio::test]
async fn rejects_file_secret_when_path_is_not_regular_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env = TestEnv::default();
    let err = resolve_secret_text(
        &format!("secret://file?path={}", dir.path().to_string_lossy()),
        &env,
    )
    .await
    .unwrap_err();

    let SecretError::Io { .. } = &err else {
        panic!("expected io error");
    };
    assert_catalog_code(
        err.structured_text(),
        "error_detail.secret.file_not_regular",
    );
    assert_catalog_text_arg(
        err.structured_text(),
        "path",
        Some(dir.path().to_string_lossy().as_ref()),
    );
}

#[cfg(unix)]
#[tokio::test]
async fn resolves_file_secret_from_scoped_symlink() -> Result<()> {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().expect("tempdir");
    let revision_dir = dir.path().join("..2026_03_18_00_00_00");
    tokio::fs::create_dir_all(&revision_dir).await?;
    tokio::fs::write(revision_dir.join("secret.txt"), "linked").await?;
    symlink(&revision_dir, dir.path().join("..data")).expect("create data symlink");
    let link = dir.path().join("secret.txt");
    symlink("..data/secret.txt", &link).expect("create secret symlink");

    let env = TestEnv::default();
    let value = resolve_secret_text(
        &format!("secret://file?path={}", link.to_string_lossy()),
        &env,
    )
    .await?;

    assert_eq!(value, "linked");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_file_secret_when_symlink_escapes_parent_tree() {
    use std::os::unix::fs::symlink;

    let outside = tempfile::tempdir().expect("outside tempdir");
    let escape_target = outside.path().join("secret.txt");
    tokio::fs::write(&escape_target, "outside")
        .await
        .expect("write outside target");

    let dir = tempfile::tempdir().expect("tempdir");
    let link = dir.path().join("secret-link.txt");
    symlink(&escape_target, &link).expect("create escape symlink");

    let env = TestEnv::default();
    let err = resolve_secret_text(
        &format!("secret://file?path={}", link.to_string_lossy()),
        &env,
    )
    .await
    .unwrap_err();

    let SecretError::Io { .. } = &err else {
        panic!("expected io error");
    };
    assert_catalog_code(
        err.structured_text(),
        "error_detail.secret.file_not_regular",
    );
    assert_catalog_text_arg(
        err.structured_text(),
        "path",
        Some(link.to_string_lossy().as_ref()),
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_file_secret_when_ancestor_directory_is_symlink() {
    use std::os::unix::fs::symlink;

    let outside = tempfile::tempdir().expect("outside tempdir");
    let target = outside.path().join("secret.txt");
    tokio::fs::write(&target, "outside")
        .await
        .expect("write outside target");

    let dir = tempfile::tempdir().expect("tempdir");
    let linked_dir = dir.path().join("linked");
    symlink(outside.path(), &linked_dir).expect("create linked directory");

    let env = TestEnv::default();
    let err = resolve_secret_text(
        &format!(
            "secret://file?path={}",
            linked_dir.join("secret.txt").to_string_lossy()
        ),
        &env,
    )
    .await
    .unwrap_err();

    let SecretError::Io { .. } = &err else {
        panic!("expected io error");
    };
    assert_catalog_code(
        err.structured_text(),
        "error_detail.secret.file_not_regular",
    );
    assert_catalog_text_arg(
        err.structured_text(),
        "path",
        Some(linked_dir.join("secret.txt").to_string_lossy().as_ref()),
    );
}

#[tokio::test]
async fn read_limited_returns_after_limit_without_waiting_for_eof() {
    use tokio::io::AsyncWriteExt as _;

    let (mut writer, reader) = tokio::io::duplex(32);
    let writer_task = tokio::spawn(async move {
        writer
            .write_all(b"overflow!")
            .await
            .expect("write test bytes");
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let (out, truncated) =
        tokio::time::timeout(Duration::from_millis(100), read_limited(reader, 8))
            .await
            .expect("read should stop at the limit")
            .expect("read should succeed");
    assert_eq!(out.as_ref(), b"overflow");
    assert!(truncated);

    writer_task.abort();
    let _ = writer_task.await;
}

#[tokio::test]
async fn resolves_file_secret_from_percent_encoded_query_path() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret file.txt");
    tokio::fs::write(&path, "encoded").await?;
    let env = TestEnv::default();
    let spec = format!(
        "secret://file?path={}",
        path.to_string_lossy().replace(' ', "%20")
    );
    let value = resolve_secret_text(&spec, &env).await?;
    assert_eq!(value, "encoded");
    Ok(())
}

#[tokio::test]
async fn resolves_file_secret_from_percent_encoded_tail_path() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tail file.txt");
    tokio::fs::write(&path, "tail").await?;
    let env = TestEnv::default();
    let spec = format!(
        "secret://file/{}",
        path.to_string_lossy().replace(' ', "%20")
    );
    let value = resolve_secret_text(&spec, &env).await?;
    assert_eq!(value, "tail");
    Ok(())
}

#[tokio::test]
async fn resolves_file_secret_from_query_path_with_literal_plus() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret+file.txt");
    tokio::fs::write(&path, "plus").await?;
    let env = TestEnv::default();
    let spec = format!("secret://file?path={}", path.to_string_lossy());
    let value = resolve_secret_text(&spec, &env).await?;
    assert_eq!(value, "plus");
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn resolves_file_secret_with_percent_encoded_trailing_space() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret ");
    tokio::fs::write(&path, "space").await?;
    let env = TestEnv::default();
    let spec = format!(
        "secret://file?path={}",
        path.to_string_lossy().replace(' ', "%20")
    );
    let value = resolve_secret_text(&spec, &env).await?;
    assert_eq!(value, "space");
    Ok(())
}

#[test]
fn parses_command_specs() -> Result<()> {
    let spec =
        SecretSpec::parse("secret://aws-sm/mysecret?region=us-east-1&profile=dev&json_key=token")?;
    let cmd = build_secret_command(&spec).expect("command");
    assert_eq!(cmd.program, "aws");
    assert!(cmd.args.iter().any(|arg| arg == "secretsmanager"));
    assert_eq!(cmd.json_key.as_deref(), Some("token"));

    let spec = SecretSpec::parse("secret://azure-kv/myvault/mysecret")?;
    let cmd = build_secret_command(&spec).expect("command");
    assert_eq!(cmd.program, "az");

    let spec = SecretSpec::parse("secret://vault/secret/openai?field=api_key&namespace=team")?;
    let cmd = build_secret_command(&spec).expect("command");
    assert_eq!(cmd.program, "vault");
    assert_eq!(
        cmd.env.get("VAULT_NAMESPACE").map(String::as_str),
        Some("team")
    );
    Ok(())
}

#[cfg(feature = "system-keyring")]
#[test]
fn parses_keyring_spec_without_command_bridge() -> Result<()> {
    let spec = SecretSpec::parse("secret://keyring/com.omne42.typemic/openai-api-key")?;
    assert!(build_secret_command(&spec).is_none());
    assert_eq!(
        spec.to_string(),
        "secret://keyring/com.omne42.typemic/openai-api-key"
    );
    Ok(())
}

#[test]
fn secret_command_debug_redacts_args_and_env_values() {
    let cmd = SecretCommand {
        program: "vault".to_string(),
        args: vec![
            "kv".to_string(),
            "get".to_string(),
            "top-secret-arg".to_string(),
        ],
        env: BTreeMap::from([("VAULT_TOKEN".to_string(), "top-secret-env".to_string())]),
        json_key: Some("token".to_string()),
    };

    let rendered = format!("{cmd:?}");

    assert!(rendered.contains("SecretCommand"));
    assert!(rendered.contains("program: \"vault\""));
    assert!(rendered.contains("arg_count: 3"));
    assert!(rendered.contains("env_keys: [\"VAULT_TOKEN\"]"));
    assert!(rendered.contains("json_key: Some(\"token\")"));
    assert!(!rendered.contains("top-secret-arg"));
    assert!(!rendered.contains("top-secret-env"));
}

#[test]
fn secret_spec_debug_redacts_sensitive_values() -> Result<()> {
    let file_path = std::env::temp_dir().join("top-secret.txt");
    let specs = [
        SecretSpec::parse("secret://env/TOP_SECRET")?,
        SecretSpec::parse(&format!(
            "secret://file?path={}",
            file_path.to_string_lossy()
        ))?,
        SecretSpec::parse("secret://vault/secret/demo?field=token&namespace=team-secret")?,
    ];

    let rendered = specs
        .iter()
        .map(|spec| format!("{spec:?}"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("SecretSpec::Env"));
    assert!(rendered.contains("SecretSpec::File"));
    assert!(rendered.contains("SecretSpec::Vault"));
    assert!(rendered.contains("has_namespace: true"));
    assert!(!rendered.contains("TOP_SECRET"));
    assert!(!rendered.contains(file_path.to_string_lossy().as_ref()));
    assert!(!rendered.contains("secret/demo"));
    assert!(!rendered.contains("team-secret"));
    assert!(!rendered.contains("token"));
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_returns_exact_stdout() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "printf ok".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let value = run_secret_command(&cmd, &env).await?;
    assert_eq!(value.expose_secret(), "ok");
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_preserves_stdout_whitespace() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "printf '  ok \\n'".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let value = run_secret_command(&cmd, &env).await?;
    assert_eq!(value.expose_secret(), "  ok \n");
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_accepts_empty_stdout() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "printf ''".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let value = run_secret_command(&cmd, &env).await?;
    assert_eq!(value.expose_secret(), "");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn secret_command_runner_returns_exact_stdout() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "cmd".to_string(),
        args: vec!["/C".to_string(), "<nul set /p =ok".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let value = run_secret_command(&cmd, &env).await?;
    assert_eq!(value.expose_secret(), "ok");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn secret_command_runner_accepts_empty_stdout() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "cmd".to_string(),
        args: vec!["/C".to_string(), "exit /B 0".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let value = run_secret_command(&cmd, &env).await?;
    assert_eq!(value.expose_secret(), "");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_secret_uses_command_env_context() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault_path = dir.path().join("vault");
    write_executable_script(&vault_path, "#!/bin/sh\nprintf '%s' \"$VAULT_ADDR\"\n")?;

    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            vault_path.to_string_lossy().into_owned(),
        )]),
        command_vars: BTreeMap::from([(
            "VAULT_ADDR".to_string(),
            "https://vault.internal".to_string(),
        )]),
        ..TestEnv::default()
    };

    let value = resolve_secret_text("secret://vault/secret/demo?field=token", &env).await?;
    assert_eq!(value, "https://vault.internal");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_secret_uses_filtered_command_env_snapshot_for_builtin_providers() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault_path = dir.path().join("vault");
    write_executable_script(
        &vault_path,
        "#!/bin/sh\nif [ \"${PATH:-missing}\" = \"poisoned\" ]; then path_state=poisoned; else path_state=safe; fi\nprintf '%s|%s|%s' \"${FOO:-missing}\" \"${VAULT_ADDR:-missing}\" \"$path_state\"\n",
    )?;

    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            vault_path.to_string_lossy().into_owned(),
        )]),
        command_vars: BTreeMap::from([
            ("FOO".to_string(), "leak".to_string()),
            (
                "VAULT_ADDR".to_string(),
                "https://vault.internal".to_string(),
            ),
            ("PATH".to_string(), "poisoned".to_string()),
        ]),
        ..TestEnv::default()
    };

    let value = resolve_secret_text("secret://vault/secret/demo?field=token", &env).await?;
    let parts = value.trim_end_matches('\n').split('|').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "missing");
    assert_eq!(parts[1], "https://vault.internal");
    assert_eq!(parts[2], "safe");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn secret_command_runner_retries_text_file_busy_spawn() -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("tempdir");
    let vault_path = dir.path().join("vault");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&vault_path)?;
    file.write_all(b"#!/bin/sh\nprintf ok\n")?;
    file.sync_all()?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o755);
    file.set_permissions(permissions)?;
    file.sync_all()?;

    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        drop(file);
    });

    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            vault_path.to_string_lossy().into_owned(),
        )]),
        ..TestEnv::default()
    };

    let value = resolve_secret_text("secret://vault/secret/demo?field=token", &env).await?;
    writer.join().expect("join writer thread");
    assert_eq!(value, "ok");
    Ok(())
}

#[cfg(unix)]
#[test]
fn resolve_program_on_path_returns_absolute_match() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("vault");
    write_executable_script(&path, "#!/bin/sh\nexit 0\n").expect("write executable");

    let resolved = resolve_program_on_path_for_test("vault", dir.path().as_os_str())
        .expect("program should resolve from explicit PATH fragment");

    assert_eq!(resolved, path);
    assert!(resolved.is_absolute());
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn resolve_program_on_path_preserves_non_utf8_absolute_match() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let parent = tempfile::tempdir().expect("tempdir");
    let non_utf8_dir = parent
        .path()
        .join(std::path::PathBuf::from(OsString::from_vec(
            b"secret-kit-non-utf8-\xff".to_vec(),
        )));
    std::fs::create_dir(&non_utf8_dir).expect("create non-utf8 directory");

    let path = non_utf8_dir.join("vault");
    write_executable_script(&path, "#!/bin/sh\nexit 0\n").expect("write executable");

    let resolved = resolve_program_on_path_for_test("vault", non_utf8_dir.as_os_str())
        .expect("program should resolve from non-utf8 PATH fragment");

    assert_eq!(resolved, path);
}

#[cfg(unix)]
#[test]
fn resolve_program_on_path_ignores_relative_search_entries() {
    let cwd = std::env::current_dir().expect("cwd");
    let relative_dir = tempfile::Builder::new()
        .prefix("secret-kit-relative-path-")
        .tempdir_in(&cwd)
        .expect("tempdir in cwd");
    let relative_basename = relative_dir
        .path()
        .file_name()
        .expect("tempdir basename")
        .to_owned();
    let relative_program = relative_dir.path().join("vault");
    write_executable_script(&relative_program, "#!/bin/sh\nexit 0\n").expect("write executable");

    let absolute_dir = tempfile::tempdir().expect("tempdir");
    let absolute_program = absolute_dir.path().join("vault");
    write_executable_script(&absolute_program, "#!/bin/sh\nexit 0\n").expect("write executable");

    let search_path = std::env::join_paths([
        std::path::Path::new(relative_basename.as_os_str()),
        absolute_dir.path(),
    ])
    .expect("join search path");
    let resolved = resolve_program_on_path_for_test("vault", search_path.as_os_str())
        .expect("resolver should ignore relative PATH entries");

    assert_eq!(resolved, absolute_program);
}

#[cfg(unix)]
#[test]
fn resolve_program_on_path_skips_non_executable_match() {
    use std::os::unix::fs::PermissionsExt as _;

    let first_dir = tempfile::tempdir().expect("tempdir");
    let second_dir = tempfile::tempdir().expect("tempdir");

    let blocked = first_dir.path().join("vault");
    std::fs::write(&blocked, "not executable").expect("write blocked candidate");
    let mut permissions = std::fs::metadata(&blocked)
        .expect("read blocked metadata")
        .permissions();
    permissions.set_mode(0o644);
    std::fs::set_permissions(&blocked, permissions).expect("set blocked permissions");

    let executable = second_dir.path().join("vault");
    write_executable_script(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");

    let search_path =
        std::env::join_paths([first_dir.path(), second_dir.path()]).expect("join search path");
    let resolved = resolve_program_on_path_for_test("vault", search_path.as_os_str())
        .expect("resolver should skip non-executable candidate");

    assert_eq!(resolved, executable);
}

#[cfg(windows)]
#[test]
fn resolve_program_on_path_uses_snapshot_pathext() {
    use std::ffi::OsStr;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("vault.cmd");
    std::fs::write(&path, "@echo off\r\n").expect("write command shim");

    let resolved = crate::command::resolve_program_on_path_with_extensions_for_test(
        "vault",
        dir.path().as_os_str(),
        Some(OsStr::new(".CMD")),
    )
    .expect("program should resolve through snapshot PATHEXT");
    assert_eq!(resolved, path);

    let missing = crate::command::resolve_program_on_path_with_extensions_for_test(
        "vault",
        dir.path().as_os_str(),
        Some(OsStr::new(".EXE")),
    );
    assert_eq!(missing, None);
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_secret_rejects_untrusted_ambient_snapshot_path_for_builtin_resolution()
-> Result<()> {
    use std::ffi::OsString;

    struct AmbientPathEnv {
        path: OsString,
    }

    impl SecretEnvironment for AmbientPathEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for AmbientPathEnv {
        fn ambient_command_env_os_pairs(
            &self,
            _program: &str,
        ) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
            Box::new(std::iter::once((OsString::from("PATH"), self.path.clone())))
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let vault_path = dir.path().join("vault");
    write_executable_script(&vault_path, "#!/bin/sh\nprintf resolved-from-snapshot\n")?;

    let env = AmbientPathEnv {
        path: dir.path().as_os_str().to_os_string(),
    };

    let err = resolve_secret_text("secret://vault/secret/demo?field=token", &env)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(&text, "error_detail.secret.command_spawn_failed");
    assert_catalog_text_arg(&text, "program", Some("vault"));
    assert_catalog_text_arg(&text, "error", Some("vault not found on ambient PATH"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_secret_sanitizes_ambient_path_before_spawning_builtin_override() -> Result<()> {
    use std::ffi::OsString;

    struct AmbientPathEnv {
        path: OsString,
        override_path: String,
    }

    impl SecretEnvironment for AmbientPathEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for AmbientPathEnv {
        fn ambient_command_env_os_pairs(
            &self,
            _program: &str,
        ) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
            Box::new(std::iter::once((OsString::from("PATH"), self.path.clone())))
        }

        fn resolve_command_program(&self, _program: &str) -> Option<String> {
            Some(self.override_path.clone())
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let vault_path = dir.path().join("vault");
    write_executable_script(&vault_path, "#!/bin/sh\nprintf '%s' \"${PATH:-missing}\"\n")?;

    let env = AmbientPathEnv {
        path: dir.path().as_os_str().to_os_string(),
        override_path: vault_path.to_string_lossy().into_owned(),
    };

    let value = resolve_secret_text("secret://vault/secret/demo?field=token", &env).await?;
    assert!(
        !value.contains(dir.path().to_string_lossy().as_ref()),
        "untrusted tempdir should not survive into child PATH: {value}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn sanitize_ambient_command_search_path_keeps_only_trusted_system_directories() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = std::env::join_paths([tempdir.path(), Path::new("/usr/bin"), Path::new("/bin")])
        .expect("join path");

    let sanitized = sanitize_ambient_command_search_path_for_test(path.as_os_str())
        .expect("trusted system directories should survive");
    let directories = std::env::split_paths(&sanitized).collect::<Vec<_>>();

    assert_eq!(directories, vec![Path::new("/usr/bin"), Path::new("/bin")]);
}

#[cfg(unix)]
#[test]
fn sanitize_ambient_command_search_path_drops_untrusted_entries() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sanitized = sanitize_ambient_command_search_path_for_test(tempdir.path().as_os_str());

    assert!(
        sanitized.is_none(),
        "tempdir should not be trusted for builtin PATH search"
    );
}

#[cfg(unix)]
#[test]
fn trusted_builtin_search_directory_metadata_rejects_missing_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing");

    assert!(
        !trusted_builtin_search_directory_metadata_for_test(&missing),
        "missing path should not be trusted"
    );
}

#[cfg(unix)]
#[test]
fn trusted_builtin_search_directory_metadata_rejects_non_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("vault");
    std::fs::write(&file, "not a directory").expect("write file");

    assert!(
        !trusted_builtin_search_directory_metadata_for_test(&file),
        "regular files should not be trusted as PATH directories"
    );
}

#[cfg(unix)]
#[test]
fn trusted_builtin_search_directory_metadata_rejects_world_writable_directory_without_sticky_bit() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut permissions = std::fs::metadata(dir.path())
        .expect("metadata")
        .permissions();
    permissions.set_mode(0o777);
    std::fs::set_permissions(dir.path(), permissions).expect("set permissions");

    assert!(
        !trusted_builtin_search_directory_metadata_for_test(dir.path()),
        "world-writable directory without sticky bit should not be trusted"
    );
}

#[cfg(unix)]
#[test]
fn trusted_builtin_search_directory_metadata_accepts_sticky_root_owned_world_writable_directory() {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut permissions = std::fs::metadata(dir.path())
        .expect("metadata")
        .permissions();
    permissions.set_mode(0o1777);
    std::fs::set_permissions(dir.path(), permissions).expect("set permissions");

    if std::fs::metadata(dir.path()).expect("metadata").uid() == 0 {
        assert!(
            trusted_builtin_search_directory_metadata_for_test(dir.path()),
            "sticky root-owned world-writable directory should remain trusted"
        );
    } else {
        assert!(
            !trusted_builtin_search_directory_metadata_for_test(dir.path()),
            "sticky directory owned by a non-root user should not be trusted"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_secret_accepts_builtin_program_override_with_absolute_path() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let override_path = dir.path().join("vault");
    write_executable_script(&override_path, "#!/bin/sh\nprintf override-ok\n")?;

    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            override_path.to_string_lossy().into_owned(),
        )]),
        ..TestEnv::default()
    };

    let value = resolve_secret_text("secret://vault/secret/demo?field=token", &env).await?;
    assert_eq!(value, "override-ok");
    Ok(())
}

#[tokio::test]
async fn resolve_secret_rejects_builtin_program_override_without_absolute_path() {
    let env = TestEnv {
        command_programs: BTreeMap::from([("vault".to_string(), "vault-wrapper".to_string())]),
        ..TestEnv::default()
    };

    let err = resolve_secret_text("secret://vault/secret/demo?field=token", &env)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(
        &text,
        "error_detail.secret.command_program_override_not_absolute",
    );
    assert_catalog_text_arg(&text, "program", Some("vault"));
}

#[tokio::test]
async fn resolve_secret_rejects_builtin_program_override_with_wrong_basename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let override_path = dir.path().join("not-vault");
    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            override_path.to_string_lossy().into_owned(),
        )]),
        ..TestEnv::default()
    };

    let err = resolve_secret_text("secret://vault/secret/demo?field=token", &env)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(
        &text,
        "error_detail.secret.command_program_override_invalid_name",
    );
    assert_catalog_text_arg(&text, "program", Some("vault"));
    assert_catalog_text_arg(
        &text,
        "resolved_program",
        Some(override_path.to_string_lossy().as_ref()),
    );
}

#[cfg(windows)]
#[tokio::test]
async fn resolve_secret_accepts_builtin_program_override_with_case_insensitive_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let override_path = dir.path().join("Vault.EXE");
    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            override_path.to_string_lossy().into_owned(),
        )]),
        ..TestEnv::default()
    };

    let err = resolve_secret_text("secret://vault/secret/demo?field=token", &env)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_ne!(
        catalog(&text).code(),
        "error_detail.secret.command_program_override_invalid_name"
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn resolve_secret_rejects_builtin_program_override_with_unix_script_suffix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let override_path = dir.path().join("vault.sh");
    let env = TestEnv {
        command_programs: BTreeMap::from([(
            "vault".to_string(),
            override_path.to_string_lossy().into_owned(),
        )]),
        ..TestEnv::default()
    };

    let err = resolve_secret_text("secret://vault/secret/demo?field=token", &env)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(
        &text,
        "error_detail.secret.command_program_override_invalid_name",
    );
    assert_catalog_text_arg(&text, "program", Some("vault"));
    assert_catalog_text_arg(
        &text,
        "resolved_program",
        Some(override_path.to_string_lossy().as_ref()),
    );
}

#[test]
fn build_command_env_keeps_gcloud_boto_configuration() {
    let env = build_command_env(
        "gcloud",
        BTreeMap::from([
            ("BOTO_CONFIG".to_string(), "/tmp/boto.cfg".to_string()),
            (
                "CLOUDSDK_ACTIVE_CONFIG_NAME".to_string(),
                "sandbox".to_string(),
            ),
            ("FOO".to_string(), "drop-me".to_string()),
        ]),
    );

    assert_eq!(
        env.get("BOTO_CONFIG").map(String::as_str),
        Some("/tmp/boto.cfg")
    );
    assert_eq!(
        env.get("CLOUDSDK_ACTIVE_CONFIG_NAME").map(String::as_str),
        Some("sandbox")
    );
    assert!(!env.contains_key("FOO"));
}

#[test]
fn build_command_env_does_not_read_ambient_path_when_snapshot_omits_it() {
    let env = build_command_env(
        "vault",
        BTreeMap::from([(
            "VAULT_ADDR".to_string(),
            "https://vault.internal".to_string(),
        )]),
    );

    assert_eq!(
        env.get("VAULT_ADDR").map(String::as_str),
        Some("https://vault.internal")
    );
    assert!(!env.contains_key("PATH"));
    assert!(!env.contains_key("Path"));
}

#[test]
fn build_command_env_keeps_azure_managed_identity_variables() {
    let env = build_command_env(
        "az",
        BTreeMap::from([
            (
                "IDENTITY_ENDPOINT".to_string(),
                "http://localhost/identity".to_string(),
            ),
            ("IDENTITY_HEADER".to_string(), "secret".to_string()),
            (
                "MSI_ENDPOINT".to_string(),
                "http://localhost/msi".to_string(),
            ),
            ("FOO".to_string(), "drop-me".to_string()),
        ]),
    );

    assert_eq!(
        env.get("IDENTITY_ENDPOINT").map(String::as_str),
        Some("http://localhost/identity")
    );
    assert_eq!(
        env.get("IDENTITY_HEADER").map(String::as_str),
        Some("secret")
    );
    assert_eq!(
        env.get("MSI_ENDPOINT").map(String::as_str),
        Some("http://localhost/msi")
    );
    assert!(!env.contains_key("FOO"));
}

#[test]
fn build_command_env_keeps_windows_process_basics() {
    let env = build_command_env(
        "az",
        BTreeMap::from([
            ("SystemRoot".to_string(), "C:\\Windows".to_string()),
            (
                "ComSpec".to_string(),
                "C:\\Windows\\System32\\cmd.exe".to_string(),
            ),
            ("PATHEXT".to_string(), ".EXE;.CMD".to_string()),
            ("WINDIR".to_string(), "C:\\Windows".to_string()),
            ("FOO".to_string(), "drop-me".to_string()),
        ]),
    );

    assert_eq!(
        env.get("SystemRoot").map(String::as_str),
        Some("C:\\Windows")
    );
    assert_eq!(
        env.get("ComSpec").map(String::as_str),
        Some("C:\\Windows\\System32\\cmd.exe")
    );
    assert_eq!(env.get("PATHEXT").map(String::as_str), Some(".EXE;.CMD"));
    assert_eq!(env.get("WINDIR").map(String::as_str), Some("C:\\Windows"));
    assert!(!env.contains_key("FOO"));
}

#[cfg(windows)]
#[test]
fn build_command_env_matches_windows_variables_case_insensitively() {
    let env = build_command_env(
        "az",
        BTreeMap::from([
            ("path".to_string(), "C:\\Windows\\System32".to_string()),
            ("azure_tenant_id".to_string(), "tenant".to_string()),
            (
                "identity_endpoint".to_string(),
                "http://localhost/identity".to_string(),
            ),
            ("foo".to_string(), "drop-me".to_string()),
        ]),
    );

    assert_eq!(
        env.get("path").map(String::as_str),
        Some("C:\\Windows\\System32")
    );
    assert_eq!(
        env.get("azure_tenant_id").map(String::as_str),
        Some("tenant")
    );
    assert_eq!(
        env.get("identity_endpoint").map(String::as_str),
        Some("http://localhost/identity")
    );
    assert!(!env.contains_key("foo"));
}

#[cfg(unix)]
#[tokio::test]
async fn run_secret_command_uses_only_explicit_command_environment() -> Result<()> {
    struct EmptyCommandEnv;

    impl SecretEnvironment for EmptyCommandEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for EmptyCommandEnv {
        fn get_command_env(&self, _key: &str) -> Option<String> {
            None
        }

        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::empty())
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("print-home");
    write_executable_script(
        &script_path,
        "#!/bin/sh\nprintf '%s' \"${HOME:-missing}\"\n",
    )?;

    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec![script_path.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        json_key: None,
    };

    let value = run_secret_command(&cmd, &EmptyCommandEnv).await?;
    assert_eq!(value.expose_secret(), "missing");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn run_secret_command_uses_only_explicit_command_environment() -> Result<()> {
    struct EmptyCommandEnv;

    impl SecretEnvironment for EmptyCommandEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for EmptyCommandEnv {
        fn get_command_env(&self, _key: &str) -> Option<String> {
            None
        }

        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::empty())
        }
    }

    let cmd = SecretCommand {
        program: "cmd".to_string(),
        args: vec![
            "/V:ON".to_string(),
            "/C".to_string(),
            "if defined USERPROFILE (<nul set /p =present) else (<nul set /p =missing)".to_string(),
        ],
        env: BTreeMap::new(),
        json_key: None,
    };

    let value = run_secret_command(&cmd, &EmptyCommandEnv).await?;
    assert_eq!(value.expose_secret(), "missing");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn run_secret_command_accepts_custom_non_whitelisted_environment() -> Result<()> {
    struct CustomCommandEnv;

    impl SecretEnvironment for CustomCommandEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for CustomCommandEnv {
        fn get_command_env(&self, key: &str) -> Option<String> {
            (key == "FOO").then(|| "from-env-object".to_string())
        }

        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::once((
                "FOO".to_string(),
                "from-env-object".to_string(),
            )))
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("print-foo");
    write_executable_script(&script_path, "#!/bin/sh\nprintf '%s' \"${FOO:-missing}\"\n")?;

    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec![script_path.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        json_key: None,
    };

    let value = run_secret_command(&cmd, &CustomCommandEnv).await?;
    assert_eq!(value.expose_secret(), "from-env-object");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn run_secret_command_accepts_custom_non_whitelisted_environment() -> Result<()> {
    struct CustomCommandEnv;

    impl SecretEnvironment for CustomCommandEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for CustomCommandEnv {
        fn get_command_env(&self, key: &str) -> Option<String> {
            (key == "FOO").then(|| "from-env-object".to_string())
        }

        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::once((
                "FOO".to_string(),
                "from-env-object".to_string(),
            )))
        }
    }

    let cmd = SecretCommand {
        program: "cmd".to_string(),
        args: vec![
            "/V:ON".to_string(),
            "/C".to_string(),
            "if defined FOO (<nul set /p =!FOO!) else (<nul set /p =missing)".to_string(),
        ],
        env: BTreeMap::new(),
        json_key: None,
    };

    let value = run_secret_command(&cmd, &CustomCommandEnv).await?;
    assert_eq!(value.expose_secret(), "from-env-object");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn run_secret_command_accepts_non_utf8_command_environment() -> Result<()> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    struct NonUtf8CommandEnv;

    impl SecretEnvironment for NonUtf8CommandEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for NonUtf8CommandEnv {
        fn command_env_os_pairs(&self) -> Box<dyn Iterator<Item = (OsString, OsString)> + '_> {
            Box::new(std::iter::once((
                OsString::from("SSL_CERT_FILE"),
                OsString::from_vec(b"/tmp/\xffcert".to_vec()),
            )))
        }
    }

    let cmd = SecretCommand {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "[ -n \"${SSL_CERT_FILE+x}\" ] && printf present || printf missing".to_string(),
        ],
        env: BTreeMap::new(),
        json_key: None,
    };

    let value = run_secret_command(&cmd, &NonUtf8CommandEnv).await?;
    assert_eq!(value.expose_secret(), "present");
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn run_secret_command_uses_single_command_env_snapshot_for_timeout() -> Result<()> {
    struct SnapshotTimeoutEnv;

    impl SecretEnvironment for SnapshotTimeoutEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for SnapshotTimeoutEnv {
        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::once((
                SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
                "10".to_string(),
            )))
        }
    }

    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 1".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &SnapshotTimeoutEnv)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(&text, "error_detail.secret.command_timeout");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn run_secret_command_reuses_explicit_command_env_snapshot_for_child_process() -> Result<()> {
    struct FlakySnapshotEnv {
        calls: AtomicUsize,
    }

    impl SecretEnvironment for FlakySnapshotEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for FlakySnapshotEnv {
        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let value = if call == 0 { "first" } else { "second" };
            Box::new(
                [
                    (
                        SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
                        "1000".to_string(),
                    ),
                    ("FOO".to_string(), value.to_string()),
                ]
                .into_iter(),
            )
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("print-foo");
    write_executable_script(&script_path, "#!/bin/sh\nprintf '%s' \"${FOO:-missing}\"\n")?;

    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec![script_path.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let env = FlakySnapshotEnv {
        calls: AtomicUsize::new(0),
    };

    let value = run_secret_command(&cmd, &env).await?;

    assert_eq!(value.expose_secret(), "first");
    assert_eq!(env.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn run_secret_command_uses_single_command_env_snapshot_for_timeout() -> Result<()> {
    struct SnapshotTimeoutEnv;

    impl SecretEnvironment for SnapshotTimeoutEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for SnapshotTimeoutEnv {
        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::once((
                SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
                "10".to_string(),
            )))
        }
    }

    let cmd = SecretCommand {
        program: "cmd".to_string(),
        args: vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &SnapshotTimeoutEnv)
        .await
        .unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(&text, "error_detail.secret.command_timeout");
    Ok(())
}

#[test]
fn run_secret_command_returns_error_without_tokio_time_driver() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        let cmd = SecretCommand {
            program: if cfg!(windows) {
                "cmd".to_string()
            } else {
                "sh".to_string()
            },
            args: if cfg!(windows) {
                vec!["/C".to_string(), "echo hi".to_string()]
            } else {
                vec!["-c".to_string(), "printf hi".to_string()]
            },
            env: BTreeMap::new(),
            json_key: None,
        };

        let env = TestEnv::default();
        let err = run_secret_command(&cmd, &env)
            .await
            .expect_err("missing time driver should fail");
        let SecretError::Command(text) = err else {
            panic!("expected secret command error");
        };
        assert_catalog_code(
            &text,
            "error_detail.secret.command_runtime_missing_time_driver",
        );
    });
}

#[test]
fn default_get_command_env_os_reads_explicit_command_env_pairs() {
    use std::ffi::OsStr;

    struct PairBackedEnv;

    impl SecretEnvironment for PairBackedEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for PairBackedEnv {
        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::once((
                SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
                "25".to_string(),
            )))
        }
    }

    let value = PairBackedEnv.get_command_env_os(OsStr::new(SECRET_COMMAND_TIMEOUT_MS_ENV));

    assert_eq!(value.as_deref(), Some(OsStr::new("25")));
}

#[cfg(windows)]
#[test]
fn default_get_command_env_os_matches_windows_variable_names_case_insensitively() {
    use std::ffi::OsStr;

    struct PairBackedEnv;

    impl SecretEnvironment for PairBackedEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for PairBackedEnv {
        fn command_env_pairs(&self) -> Box<dyn Iterator<Item = (String, String)> + '_> {
            Box::new(std::iter::once((
                SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
                "25".to_string(),
            )))
        }
    }

    let value = PairBackedEnv.get_command_env_os(OsStr::new("secret_command_timeout_ms"));

    assert_eq!(value.as_deref(), Some(OsStr::new("25")));
}

#[test]
fn default_get_command_env_os_does_not_fallback_when_custom_env_returns_none() {
    use std::ffi::OsStr;

    struct NoCommandEnv;

    impl SecretEnvironment for NoCommandEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for NoCommandEnv {
        fn get_command_env(&self, _key: &str) -> Option<String> {
            None
        }
    }

    let key = ["PATH", "Path", "HOME", "USERPROFILE", "TMPDIR", "TEMP"]
        .into_iter()
        .find(|key| std::env::var_os(key).is_some())
        .expect("test environment should expose at least one common process variable");

    let value = NoCommandEnv.get_command_env_os(OsStr::new(key));

    assert_eq!(value, None);
}

#[test]
fn secret_command_timeout_ignores_get_command_env_fallback() {
    struct LookupOnlyTimeoutEnv;

    impl SecretEnvironment for LookupOnlyTimeoutEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    impl SecretCommandRuntime for LookupOnlyTimeoutEnv {
        fn get_command_env(&self, key: &str) -> Option<String> {
            (key == SECRET_COMMAND_TIMEOUT_MS_ENV).then(|| "10".to_string())
        }
    }

    assert_eq!(
        secret_command_timeout_from_env(&LookupOnlyTimeoutEnv),
        Duration::from_secs(DEFAULT_SECRET_COMMAND_TIMEOUT_SECS)
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_rejects_non_utf8_stdout() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "printf '\\377'".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &env).await.unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(&text, "error_detail.secret.command_stdout_not_utf8");
    assert_catalog_text_arg(&text, "program", Some("sh"));
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_rejects_oversized_stdout_before_timeout() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([(
            SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
            "1000".to_string(),
        )]),
        ..TestEnv::default()
    };
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            format!(
                "yes a | head -c {}; sleep 30",
                MAX_SECRET_COMMAND_OUTPUT_BYTES + 1
            ),
        ],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &env).await.unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(&text, "error_detail.secret.command_stdout_too_large");
    assert_catalog_text_arg(
        &text,
        "max_bytes",
        Some(MAX_SECRET_COMMAND_OUTPUT_BYTES.to_string().as_str()),
    );
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_times_out() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([("SECRET_COMMAND_TIMEOUT_MS".to_string(), "10".to_string())]),
        ..TestEnv::default()
    };
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 1; echo ok".to_string()],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &env).await.unwrap_err();
    let SecretError::Command(text) = err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(&text, "error_detail.secret.command_timeout");
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_discards_stderr_from_errors() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "echo leaked-secret >&2; exit 1".to_string(),
        ],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &env).await.unwrap_err();
    let rendered = err.to_string();
    let SecretError::Command(text) = &err else {
        panic!("expected secret command error");
    };
    assert_catalog_code(text, "error_detail.secret.command_failed_status");
    assert_eq!(catalog(text).unsigned_arg("stderr_bytes"), Some(14));
    assert_catalog_arg_missing(text, "stderr_hint");
    assert!(!rendered.contains("leaked-secret"));
    assert!(!rendered.contains("stderr="));
    assert!(rendered.contains("stderr_bytes=14"));
    assert!(rendered.contains("error_detail.secret.command_failed_status"));
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test]
async fn secret_command_runner_classifies_safe_stderr_failures() -> Result<()> {
    let env = TestEnv::default();
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "echo Permission denied >&2; exit 1".to_string(),
        ],
        env: BTreeMap::new(),
        json_key: None,
    };
    let err = run_secret_command(&cmd, &env).await.unwrap_err();
    let SecretError::Command(text) = &err else {
        panic!("expected secret command error");
    };

    assert_catalog_code(text, "error_detail.secret.command_failed_status");
    assert_eq!(catalog(text).text_arg("stderr_hint"), Some("auth"));
    assert_eq!(catalog(text).unsigned_arg("stderr_bytes"), Some(18));
    Ok(())
}

#[test]
fn stderr_classifier_detects_multiple_failure_families_case_insensitively() {
    assert_eq!(
        crate::command::classify_command_stderr(b"PERMISSION DENIED\n"),
        Some("auth")
    );
    assert_eq!(
        crate::command::classify_command_stderr(b"Resource NOT FOUND\n"),
        Some("not_found")
    );
    assert_eq!(
        crate::command::classify_command_stderr(b"Connection refused by upstream\n"),
        Some("network")
    );
    assert_eq!(
        crate::command::classify_command_stderr(b"Context deadline exceeded\n"),
        Some("timeout")
    );
    assert_eq!(
        crate::command::classify_command_stderr(b"Too Many Requests\n"),
        Some("rate_limit")
    );
}

#[test]
fn stderr_classifier_keeps_unknown_text_unclassified() {
    assert_eq!(
        crate::command::classify_command_stderr(b"plain failure"),
        None
    );
}

#[tokio::test]
async fn caching_resolver_reuses_successful_values() -> Result<()> {
    let resolver = CachingSecretResolver::new(CountingResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-1");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn legacy_cache_aware_resolver_still_supports_parsed_secret_calls() -> Result<()> {
    let resolver =
        CachingSecretResolver::new(LegacyCountingResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;
    let spec = SecretSpec::parse(&spec)?;

    let first = resolver.resolve_secret_spec_text(&spec, &env).await?;
    let second = resolver.resolve_secret_spec_text(&spec, &env).await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-1");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn default_secret_resolver_rejects_relative_file_specs() {
    let err = resolve_secret_text("secret://file?path=relative.txt", &TestEnv::default())
        .await
        .unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.file_path_must_be_absolute");
    assert_catalog_text_arg(&text, "path", Some("relative.txt"));
}

#[tokio::test]
async fn caching_resolver_does_not_cache_env_specs() -> Result<()> {
    let resolver = CachingSecretResolver::new(CountingResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver
        .resolve_secret_text("secret://env/TEST", &env)
        .await?;
    let second = resolver
        .resolve_secret_text("secret://env/TEST", &env)
        .await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_expires_entries_after_ttl() -> Result<()> {
    let resolver =
        CachingSecretResolver::new(CountingResolver::default(), Duration::from_millis(10));
    let env = TestEnv::default();
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_does_not_cache_errors() -> Result<()> {
    let resolver = CachingSecretResolver::new(RetryResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;

    assert!(resolver.resolve_secret_text(&spec, &env).await.is_err());
    let recovered = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(recovered, "recovered");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_coalesces_concurrent_misses() -> Result<()> {
    let resolver = CachingSecretResolver::new(SlowResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;

    let first = async { resolver.resolve_secret_text(&spec, &env).await };
    let second = async { resolver.resolve_secret_text(&spec, &env).await };
    let (first, second) = tokio::join!(first, second);

    assert_eq!(first?, "value-1");
    assert_eq!(second?, "value-1");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_ignores_mismatched_hint_scopes() -> Result<()> {
    let resolver =
        CachingSecretResolver::new(MismatchedHintResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let spec_a = test_env_spec("SPEC_A");
    let spec_b = test_env_spec("SPEC_B");
    let first = resolver.resolve_secret_text(&spec_a, &env).await?;
    let second = resolver.resolve_secret_text(&spec_a, &env).await?;
    let third = resolver.resolve_secret_text(&spec_b, &env).await?;

    assert_eq!(first, "SPEC_A-1");
    assert_eq!(second, "SPEC_A-1");
    assert_eq!(third, "SPEC_B-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_validates_hint_hits_against_prepared_cache_key() -> Result<()> {
    let resolver =
        CachingSecretResolver::new(MixedHintResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let spec_a = test_env_spec("SPEC_A");
    let spec_b = test_env_spec("SPEC_B");
    let first = resolver.resolve_secret_text(&spec_a, &env).await?;
    let second = resolver.resolve_secret_text(&spec_b, &env).await?;
    let third = resolver.resolve_secret_text(&spec_a, &env).await?;

    assert_eq!(first, "SPEC_A-1");
    assert_eq!(second, "SPEC_B-2");
    assert_eq!(third, "SPEC_A-1");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_does_not_serialize_distinct_specs_on_mismatched_hint() -> Result<()> {
    let resolver = CachingSecretResolver::new(
        SlowMismatchedHintResolver::default(),
        Duration::from_secs(60),
    );
    let env = TestEnv::default();

    let spec_a = test_env_spec("SPEC_A");
    let spec_b = test_env_spec("SPEC_B");
    let first = resolver.resolve_secret_text(&spec_a, &env);
    let second = resolver.resolve_secret_text(&spec_b, &env);
    let (first, second) = tokio::join!(first, second);

    assert_eq!(first?, "SPEC_A");
    assert_eq!(second?, "SPEC_B");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    assert!(
        resolver.inner().max_active.load(Ordering::SeqCst) >= 2,
        "distinct specs should not block each other behind a mismatched hint gate"
    );
    Ok(())
}

#[tokio::test]
async fn caching_resolver_partitions_cache_by_environment_partition() -> Result<()> {
    let resolver = CachingSecretResolver::new(
        EnvironmentScopedResolver::default(),
        Duration::from_secs(60),
    );
    let env_a = TestEnv {
        cache_partition: "env-a".to_string(),
        vars: BTreeMap::from([("API_KEY".to_string(), "prod".to_string())]),
        ..TestEnv::default()
    };
    let env_b = TestEnv {
        cache_partition: "env-b".to_string(),
        vars: BTreeMap::from([("API_KEY".to_string(), "staging".to_string())]),
        ..TestEnv::default()
    };

    let spec = test_env_spec("API_KEY");
    let first = resolver.resolve_secret_text(&spec, &env_a).await?;
    let second = resolver.resolve_secret_text(&spec, &env_b).await?;
    let third = resolver.resolve_secret_text(&spec, &env_a).await?;

    assert_eq!(first, "prod");
    assert_eq!(second, "staging");
    assert_eq!(third, "prod");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_reuses_cache_across_equivalent_environment_instances() -> Result<()> {
    let resolver = CachingSecretResolver::new(
        EnvironmentScopedResolver::default(),
        Duration::from_secs(60),
    );
    let env_a = TestEnv {
        cache_partition: "shared-prod".to_string(),
        vars: BTreeMap::from([("API_KEY".to_string(), "prod".to_string())]),
        ..TestEnv::default()
    };
    let env_b = TestEnv {
        cache_partition: "shared-prod".to_string(),
        vars: BTreeMap::from([("API_KEY".to_string(), "prod".to_string())]),
        ..TestEnv::default()
    };

    let spec = test_env_spec("API_KEY");
    let first = resolver.resolve_secret_text(&spec, &env_a).await?;
    let second = resolver.resolve_secret_text(&spec, &env_b).await?;

    assert_eq!(first, "prod");
    assert_eq!(second, "prod");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_partitions_runtime_sensitive_cache_by_runtime_partition() -> Result<()> {
    let resolver =
        CachingSecretResolver::new(RuntimeScopedResolver::default(), Duration::from_secs(60));
    let environment = TestEnv {
        cache_partition: "shared-env".to_string(),
        ..TestEnv::default()
    };
    let runtime_a = TestEnv {
        runtime_cache_partition: Some("runtime-a".to_string()),
        command_vars: BTreeMap::from([("RUNTIME_SECRET".to_string(), "alpha".to_string())]),
        ..TestEnv::default()
    };
    let runtime_b = TestEnv {
        runtime_cache_partition: Some("runtime-b".to_string()),
        command_vars: BTreeMap::from([("RUNTIME_SECRET".to_string(), "bravo".to_string())]),
        ..TestEnv::default()
    };

    let first = resolver
        .resolve_secret_text_with_runtime(
            &test_env_spec("RUNTIME_SECRET"),
            &environment,
            &runtime_a,
        )
        .await?;
    let second = resolver
        .resolve_secret_text_with_runtime(
            &test_env_spec("RUNTIME_SECRET"),
            &environment,
            &runtime_b,
        )
        .await?;
    let third = resolver
        .resolve_secret_text_with_runtime(
            &test_env_spec("RUNTIME_SECRET"),
            &environment,
            &runtime_a,
        )
        .await?;

    assert_eq!(first, "alpha");
    assert_eq!(second, "bravo");
    assert_eq!(third, "alpha");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_skips_runtime_sensitive_cache_without_runtime_partition() -> Result<()> {
    let resolver =
        CachingSecretResolver::new(RuntimeScopedResolver::default(), Duration::from_secs(60));
    let environment = TestEnv {
        cache_partition: "shared-env".to_string(),
        ..TestEnv::default()
    };
    let runtime = TestEnv {
        runtime_cache_partition: None,
        command_vars: BTreeMap::from([("RUNTIME_SECRET".to_string(), "alpha".to_string())]),
        ..TestEnv::default()
    };

    let first = resolver
        .resolve_secret_text_with_runtime(&test_env_spec("RUNTIME_SECRET"), &environment, &runtime)
        .await?;
    let second = resolver
        .resolve_secret_text_with_runtime(&test_env_spec("RUNTIME_SECRET"), &environment, &runtime)
        .await?;

    assert_eq!(first, "alpha");
    assert_eq!(second, "alpha");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_skips_runtime_sensitive_cache_with_ambient_runtime() -> Result<()> {
    #[derive(Default)]
    struct AmbientRuntimeResolver {
        calls: AtomicUsize,
    }

    impl SecretResolver for AmbientRuntimeResolver {
        fn resolve_secret_spec<'a>(
            &'a self,
            spec: &'a SecretSpec,
            context: SecretResolutionContext<'a>,
        ) -> SecretResolveFuture<'a> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let value = context
                    .command_runtime()
                    .get_command_env(env_spec_key(spec))
                    .expect("ambient runtime variable should exist");
                Ok(SecretString::from(value))
            })
        }
    }

    impl CacheAwareSecretResolver for AmbientRuntimeResolver {
        type Prepared = SecretSpec;

        fn lookup_secret_cache_scope_for_spec(
            &self,
            spec: &SecretSpec,
            _context: SecretResolutionContext<'_>,
        ) -> Result<Option<String>> {
            Ok(Some(env_spec_key(spec).to_string()))
        }

        fn lookup_secret_cache_partitioning_for_spec(
            &self,
            _spec: &SecretSpec,
            _context: SecretResolutionContext<'_>,
        ) -> Option<SecretCachePartitioning> {
            Some(SecretCachePartitioning::EnvironmentAndCommandRuntime)
        }

        async fn prepare_secret_spec_resolution(
            &self,
            spec: &SecretSpec,
            _context: SecretResolutionContext<'_>,
        ) -> Result<PreparedSecretResolution<Self::Prepared>> {
            let key = env_spec_key(spec).to_string();
            Ok(PreparedSecretResolution::cached_with_partitioning(
                spec.clone(),
                key,
                SecretCachePartitioning::EnvironmentAndCommandRuntime,
            ))
        }

        async fn resolve_prepared_secret(
            &self,
            prepared: Self::Prepared,
            context: SecretResolutionContext<'_>,
        ) -> Result<SecretString> {
            self.resolve_secret_spec(&prepared, context).await
        }
    }

    let resolver =
        CachingSecretResolver::new(AmbientRuntimeResolver::default(), Duration::from_secs(60));
    let environment = TestEnv {
        cache_partition: "shared-env".to_string(),
        ..TestEnv::default()
    };

    let first = resolver
        .resolve_secret_text(&test_env_spec("PATH"), &environment)
        .await?;
    let second = resolver
        .resolve_secret_text(&test_env_spec("PATH"), &environment)
        .await?;

    assert!(!first.is_empty(), "ambient PATH should be non-empty");
    assert_eq!(first, second);
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_skips_cache_without_environment_partition() -> Result<()> {
    struct PartitionlessEnv;

    impl SecretEnvironment for PartitionlessEnv {
        fn get_secret(&self, _key: &str) -> Option<SecretString> {
            None
        }
    }

    let resolver = CachingSecretResolver::new(CountingResolver::default(), Duration::from_secs(60));
    let env = PartitionlessEnv;
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_skips_cache_with_empty_environment_partition() -> Result<()> {
    let resolver = CachingSecretResolver::new(CountingResolver::default(), Duration::from_secs(60));
    let env = TestEnv {
        cache_partition: String::new(),
        ..TestEnv::default()
    };
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn prepared_resolution_treats_empty_cache_scope_as_uncached() {
    let prepared = PreparedSecretResolution::cached((), "");

    assert!(prepared.cache_scope().is_none());
}

#[tokio::test]
async fn caching_resolver_skips_cache_with_empty_scope() -> Result<()> {
    #[derive(Default)]
    struct EmptyScopeResolver {
        calls: AtomicUsize,
    }

    impl SecretResolver for EmptyScopeResolver {
        fn resolve_secret_spec<'a>(
            &'a self,
            _spec: &'a SecretSpec,
            _context: SecretResolutionContext<'a>,
        ) -> SecretResolveFuture<'a> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(SecretString::from(format!("value-{call}")))
            })
        }
    }

    impl CacheAwareSecretResolver for EmptyScopeResolver {
        type Prepared = ();

        async fn prepare_secret_spec_resolution(
            &self,
            _spec: &SecretSpec,
            _context: SecretResolutionContext<'_>,
        ) -> Result<PreparedSecretResolution<Self::Prepared>> {
            Ok(PreparedSecretResolution::cached((), ""))
        }

        async fn resolve_prepared_secret(
            &self,
            _prepared: Self::Prepared,
            context: SecretResolutionContext<'_>,
        ) -> Result<SecretString> {
            self.resolve_secret_spec(&SecretSpec::Env { key: String::new() }, context)
                .await
        }
    }

    let resolver =
        CachingSecretResolver::new(EmptyScopeResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver
        .resolve_secret_text(&test_env_spec("EMPTY_SCOPE"), &env)
        .await?;
    let second = resolver
        .resolve_secret_text(&test_env_spec("EMPTY_SCOPE"), &env)
        .await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn caching_resolver_invalidates_when_file_metadata_changes() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.txt");
    tokio::fs::write(&path, b"v1").await?;
    let spec = format!("secret://file?path={}", path.to_string_lossy());
    let resolver = CachingSecretResolver::new(CountingResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    tokio::fs::write(&path, b"version-two").await?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn default_caching_resolver_refreshes_when_file_contents_change_without_metadata_change()
-> Result<()> {
    use std::fs::{FileTimes, OpenOptions};

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.txt");
    tokio::fs::write(&path, b"alpha1").await?;
    let original_modified = std::fs::metadata(&path)?.modified()?;
    let spec = format!("secret://file?path={}", path.to_string_lossy());
    let resolver = CachingSecretResolver::new(DefaultSecretResolver, Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    tokio::fs::write(&path, b"bravo2").await?;
    OpenOptions::new()
        .write(true)
        .open(&path)?
        .set_times(FileTimes::new().set_modified(original_modified))?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "alpha1");
    assert_eq!(second, "bravo2");
    Ok(())
}

#[tokio::test]
async fn default_prepared_resolution_keeps_absolute_files_uncached() -> Result<()> {
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;
    let spec = SecretSpec::parse(&spec)?;

    let prepared = prepare_default_secret_spec_resolution(spec);

    assert!(prepared.cache_scope().is_none());
    Ok(())
}

#[tokio::test]
async fn default_prepared_resolution_reads_file_at_resolve_time() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.txt");
    tokio::fs::write(&path, b"v1").await?;
    let spec = format!("secret://file?path={}", path.to_string_lossy());
    let spec = SecretSpec::parse(&spec)?;

    let prepared = prepare_default_secret_spec_resolution(spec);
    tokio::fs::write(&path, b"v2").await?;

    let env = TestEnv::default();
    let value = resolve_prepared_default_secret(
        prepared.into_prepared(),
        SecretResolutionContext::new(&env, &env),
    )
    .await?;

    assert_eq!(value.expose_secret(), "v2");
    Ok(())
}

#[tokio::test]
async fn default_prepared_resolution_leaves_env_specs_uncached() -> Result<()> {
    let prepared =
        prepare_default_secret_spec_resolution(SecretSpec::parse("secret://env/TEST_SECRET")?);

    assert!(prepared.cache_scope().is_none());
    Ok(())
}

#[tokio::test]
async fn default_secret_resolver_does_not_hint_file_cache_scope() -> Result<()> {
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;
    let spec = SecretSpec::parse(&spec)?;
    let env = TestEnv::default();
    let hint = DefaultSecretResolver
        .lookup_secret_cache_scope_for_spec(&spec, SecretResolutionContext::new(&env, &env))?;

    assert_eq!(hint, None);
    Ok(())
}

#[tokio::test]
async fn resolve_secret_spec_helper_resolves_parsed_secret() -> Result<()> {
    let env = TestEnv {
        vars: BTreeMap::from([("TEST_SECRET".to_string(), "ok".to_string())]),
        ..TestEnv::default()
    };
    let spec = SecretSpec::parse("secret://env/TEST_SECRET")?;

    let value = resolve_secret_spec(&spec, &env).await?;

    assert_eq!(value.expose_secret(), "ok");
    Ok(())
}

#[tokio::test]
async fn default_caching_resolver_refreshes_file_value_without_ttl_delay() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.txt");
    tokio::fs::write(&path, b"v1").await?;
    let spec = format!("secret://file?path={}", path.to_string_lossy());
    let resolver = CachingSecretResolver::new(DefaultSecretResolver, Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    tokio::fs::write(&path, b"version-two").await?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "v1");
    assert_eq!(second, "version-two");
    Ok(())
}

#[tokio::test]
async fn default_caching_resolver_does_not_wait_for_ttl_after_file_change() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.txt");
    tokio::fs::write(&path, b"v1").await?;
    let spec = format!("secret://file?path={}", path.to_string_lossy());
    let resolver = CachingSecretResolver::new(DefaultSecretResolver, Duration::from_millis(10));
    let env = TestEnv::default();

    let first = resolver.resolve_secret_text(&spec, &env).await?;
    tokio::fs::write(&path, b"version-two").await?;
    let second = resolver.resolve_secret_text(&spec, &env).await?;

    assert_eq!(first, "v1");
    assert_eq!(second, "version-two");
    Ok(())
}

#[tokio::test]
async fn caching_resolver_does_not_cache_cli_specs() -> Result<()> {
    let resolver = CachingSecretResolver::new(CountingResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver
        .resolve_secret_text("secret://aws-sm/TEST", &env)
        .await?;
    let second = resolver
        .resolve_secret_text("secret://aws-sm/TEST", &env)
        .await?;

    assert_eq!(first, "value-1");
    assert_eq!(second, "value-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn secret_command_timeout_prefers_ms_env_when_both_are_set() {
    let env = TestEnv {
        vars: BTreeMap::from([
            (SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(), "250".to_string()),
            (
                SECRET_COMMAND_TIMEOUT_SECS_ENV.to_string(),
                "30".to_string(),
            ),
        ]),
        ..TestEnv::default()
    };

    assert_eq!(
        secret_command_timeout_from_env(&env),
        Duration::from_millis(250)
    );
}

#[test]
fn secret_command_timeout_clamps_large_ms_values() {
    let env = TestEnv {
        vars: BTreeMap::from([(
            SECRET_COMMAND_TIMEOUT_MS_ENV.to_string(),
            "999999".to_string(),
        )]),
        ..TestEnv::default()
    };

    assert_eq!(
        secret_command_timeout_from_env(&env),
        Duration::from_secs(MAX_SECRET_COMMAND_TIMEOUT_SECS)
    );
}

#[test]
fn extract_json_key_reports_missing_nested_path() {
    let err = extract_json_key(r#"{"outer":{"present":"ok"}}"#, "outer.missing").unwrap_err();
    let SecretError::Lookup(text) = err else {
        panic!("expected lookup error");
    };
    assert_catalog_code(&text, "error_detail.secret.json_missing_key");
    assert_catalog_text_arg(&text, "key", Some("outer.missing"));
}

#[test]
fn extract_json_key_treats_null_as_missing() {
    let err = extract_json_key(r#"{"outer":{"token":null}}"#, "outer.token").unwrap_err();
    let SecretError::Lookup(text) = err else {
        panic!("expected lookup error");
    };
    assert_catalog_code(&text, "error_detail.secret.json_missing_key");
    assert_catalog_text_arg(&text, "key", Some("outer.token"));
}

#[test]
fn extract_json_key_rejects_invalid_path_shape() {
    let err = extract_json_key(r#"{"outer":{"token":"ok"}}"#, "outer..token").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.invalid_json_key_path");
    assert_catalog_text_arg(&text, "key", Some("outer..token"));
}

#[test]
fn extract_json_key_reports_invalid_json_as_structured_error() {
    let err = extract_json_key("{", "outer.token").unwrap_err();
    let SecretError::Json { .. } = &err else {
        panic!("expected json error");
    };
    assert_catalog_code(
        err.structured_text(),
        "error_detail.secret.json_parse_failed",
    );
    assert_catalog_text_arg(err.structured_text(), "key", Some("outer.token"));
}

#[test]
fn extract_json_key_serializes_non_string_leaf() -> Result<()> {
    let value = extract_json_key(r#"{"outer":{"token":{"value":"ok"}}}"#, "outer.token")?;
    assert_eq!(value.expose_secret(), r#"{"value":"ok"}"#);
    Ok(())
}

#[tokio::test]
async fn file_read_errors_expose_structured_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("missing.txt");
    let env = TestEnv::default();
    let err = resolve_secret_text(
        &format!("secret://file?path={}", path.to_string_lossy()),
        &env,
    )
    .await
    .unwrap_err();
    let SecretError::Io { .. } = &err else {
        panic!("expected io error");
    };
    assert_catalog_code(
        err.structured_text(),
        "error_detail.secret.file_read_failed",
    );
    assert_catalog_text_arg(
        err.structured_text(),
        "path",
        Some(path.to_string_lossy().as_ref()),
    );
}

#[test]
fn all_secret_error_variants_expose_structured_text() {
    let io_err = SecretError::from(std::io::Error::other("boom"));
    assert_catalog_code(io_err.structured_text(), "error_detail.secret.io_error");

    let json_err = serde_json::from_str::<serde_json::Value>("{")
        .map_err(SecretError::from)
        .expect_err("invalid json should fail");
    assert_catalog_code(json_err.structured_text(), "error_detail.secret.json_error");

    let lookup_err = SecretError::lookup(structured_text!("error_detail.secret.missing_env_var"));
    assert_catalog_code(
        lookup_err.structured_text(),
        "error_detail.secret.missing_env_var",
    );

    let invalid_spec = invalid_response!("error_detail.secret.scheme_missing");
    assert_catalog_code(
        invalid_spec.structured_text(),
        "error_detail.secret.scheme_missing",
    );

    let command_err = secret_command_error!("error_detail.secret.command_timeout");
    assert_catalog_code(
        command_err.structured_text(),
        "error_detail.secret.command_timeout",
    );
}

#[test]
fn secret_error_maps_to_error_record_metadata() {
    let error = SecretError::lookup(structured_text!("error_detail.secret.missing_env_var"));
    let record = error.error_record();

    assert_secret_error_code(&error, "secret.lookup");
    assert_eq!(record.code().as_str(), "secret.lookup");
    assert_eq!(record.category(), ErrorCategory::NotFound);
    assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
    assert_catalog_code(record.user_text(), "error_detail.secret.missing_env_var");
    assert!(record.source().is_none());
}

#[test]
fn secret_error_into_error_record_preserves_source() {
    let record = SecretError::from(std::io::Error::other("boom")).into_error_record();

    assert_eq!(record.code().as_str(), "secret.io");
    assert_eq!(record.category(), ErrorCategory::ExternalDependency);
    assert_eq!(record.retry_advice(), ErrorRetryAdvice::Retryable);
    assert_eq!(
        record
            .source()
            .expect("source should be present")
            .to_string(),
        "boom"
    );
}

#[test]
fn secret_spec_display_round_trips_to_parse() -> Result<()> {
    let specs = [
        SecretSpec::Env {
            key: "OPENAI API/KEY".to_string(),
        },
        SecretSpec::File {
            path: "/tmp/secret value.txt".to_string(),
        },
        SecretSpec::Vault {
            path: "secret/data/demo token".to_string(),
            field: "api key".to_string(),
            namespace: Some("team/core".to_string()),
        },
        SecretSpec::AwsSecretsManager {
            secret_id: "demo/primary".to_string(),
            region: Some("us-east-1".to_string()),
            profile: Some("prod profile".to_string()),
            json_key: Some("outer.token".to_string()),
        },
        SecretSpec::GcpSecretManager {
            secret: "demo/value".to_string(),
            project: Some("proj one".to_string()),
            version: "7".to_string(),
            json_key: Some("outer.token".to_string()),
        },
        SecretSpec::AzureKeyVault {
            vault: "vault one".to_string(),
            name: "secret/name".to_string(),
            version: Some("123".to_string()),
        },
        #[cfg(feature = "system-keyring")]
        SecretSpec::Keyring {
            service: "com.omne42.typemic".to_string(),
            account: "openai/api key".to_string(),
        },
    ];

    for spec in specs {
        let rendered = spec.to_string();
        let reparsed = SecretSpec::parse(&rendered)?;
        assert_eq!(reparsed, spec, "{rendered}");
    }

    Ok(())
}

#[test]
fn parse_rejects_invalid_percent_encoding() {
    let err = SecretSpec::parse("secret://file?path=%ZZ").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.invalid_percent_encoding");
}

#[test]
fn parse_rejects_unknown_query_parameter() {
    let err = SecretSpec::parse("secret://aws-sm/demo?jsonKey=token").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.unsupported_query_parameter");
    assert_catalog_text_arg(&text, "provider", Some("aws-sm"));
    assert_catalog_text_arg(&text, "parameter", Some("jsonKey"));
}

#[test]
fn parse_rejects_noncanonical_aws_provider_name() {
    let err = SecretSpec::parse("secret://aws-secrets-manager/demo").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.unsupported_provider");
    assert_catalog_text_arg(&text, "provider", Some("aws-secrets-manager"));
}

#[test]
fn parse_rejects_noncanonical_gcp_provider_name() {
    let err = SecretSpec::parse("secret://gcp-secret-manager/demo").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.unsupported_provider");
    assert_catalog_text_arg(&text, "provider", Some("gcp-secret-manager"));
}

#[test]
fn parse_rejects_noncanonical_azure_provider_name() {
    let err = SecretSpec::parse("secret://azure-key-vault/demo").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.unsupported_provider");
    assert_catalog_text_arg(&text, "provider", Some("azure-key-vault"));
}

#[test]
fn parse_rejects_duplicate_query_parameter() {
    let err =
        SecretSpec::parse("secret://aws-sm/demo?region=us-east-1&region=us-west-2").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.duplicate_query_parameter");
    assert_catalog_text_arg(&text, "parameter", Some("region"));
}

#[test]
fn parse_rejects_empty_query_parameter_value() {
    let err = SecretSpec::parse("secret://aws-sm/demo?json_key=").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.empty_query_parameter");
    assert_catalog_text_arg(&text, "parameter", Some("json_key"));
}

#[test]
fn parse_rejects_invalid_json_key_path() {
    let err = SecretSpec::parse("secret://aws-sm/demo?json_key=outer..token").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.invalid_json_key_path");
    assert_catalog_text_arg(&text, "key", Some("outer..token"));
}

#[test]
fn parse_rejects_conflicting_file_paths() {
    let err = SecretSpec::parse("secret://file/tmp/one?path=/tmp/two").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.file_path_conflict");
}

#[test]
fn parse_rejects_relative_file_query_path() {
    let err = SecretSpec::parse("secret://file?path=relative.txt").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.file_path_must_be_absolute");
    assert_catalog_text_arg(&text, "path", Some("relative.txt"));
}

#[test]
fn parse_rejects_relative_file_tail_path() {
    let err = SecretSpec::parse("secret://file/relative.txt").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.file_path_must_be_absolute");
    assert_catalog_text_arg(&text, "path", Some("relative.txt"));
}

#[test]
fn parse_file_tail_rejects_invalid_percent_encoding() {
    let err = SecretSpec::parse("secret://file/%ZZ").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.invalid_percent_encoding");
}

#[test]
fn parse_rejects_option_like_vault_path() {
    let err = SecretSpec::parse("secret://vault/--help?field=token").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.option_like_cli_argument");
    assert_catalog_text_arg(&text, "provider", Some("vault"));
    assert_catalog_text_arg(&text, "field", Some("path"));
}

#[test]
fn parse_rejects_option_like_gcp_version() {
    let err = SecretSpec::parse("secret://gcp-sm/demo?version=--help").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.option_like_cli_argument");
    assert_catalog_text_arg(&text, "provider", Some("gcp-sm"));
    assert_catalog_text_arg(&text, "field", Some("version"));
}

#[test]
fn parse_rejects_option_like_aws_secret_id() {
    let err = SecretSpec::parse("secret://aws-sm/--help").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.option_like_cli_argument");
    assert_catalog_text_arg(&text, "provider", Some("aws-sm"));
    assert_catalog_text_arg(&text, "field", Some("secret_id"));
}

#[test]
fn parse_rejects_option_like_gcp_project() {
    let err = SecretSpec::parse("secret://gcp-sm/demo?project=--help").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.option_like_cli_argument");
    assert_catalog_text_arg(&text, "provider", Some("gcp-sm"));
    assert_catalog_text_arg(&text, "field", Some("project"));
}

#[test]
fn parse_rejects_option_like_azure_version() {
    let err = SecretSpec::parse("secret://azure-kv/myvault/mysecret?version=--help").unwrap_err();
    let SecretError::InvalidSpec(text) = err else {
        panic!("expected invalid spec error");
    };
    assert_catalog_code(&text, "error_detail.secret.option_like_cli_argument");
    assert_catalog_text_arg(&text, "provider", Some("azure-kv"));
    assert_catalog_text_arg(&text, "field", Some("version"));
}

#[tokio::test]
async fn missing_env_secret_reports_lookup_error() {
    let err = resolve_secret_text("secret://env/MISSING_SECRET", &TestEnv::default())
        .await
        .unwrap_err();
    let SecretError::Lookup(text) = err else {
        panic!("expected lookup error");
    };
    assert_catalog_code(&text, "error_detail.secret.missing_env_var");
    assert_catalog_text_arg(&text, "key", Some("MISSING_SECRET"));
}

#[cfg(all(unix, target_os = "linux"))]
#[tokio::test]
async fn secret_command_runner_returns_after_successful_leader_exit() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell_pid_file = dir.path().join("secret-command-shell.pid");
    let shell_pgid_file = dir.path().join("secret-command-shell.pgid");
    let pid_file = dir.path().join("secret-command-background.pid");
    let script = format!(
        "echo $$ > '{shell}'; \
         awk '{{print $5}}' /proc/$$/stat > '{shell_pgid}'; \
         sleep 30 </dev/null >/dev/null 2>&1 & \
         bg=$!; \
         echo $bg > '{background}'; \
         while [ -r /proc/$bg/stat ]; do \
           bg_pgid=$(awk '{{print $5}}' /proc/$bg/stat 2>/dev/null || true); \
           shell_pgid=$(cat '{shell_pgid}' 2>/dev/null || true); \
           [ -n \"$shell_pgid\" ] && [ \"$bg_pgid\" = \"$shell_pgid\" ] && break; \
           sleep 0.01; \
         done; \
         printf ok",
        shell = shell_pid_file.display(),
        shell_pgid = shell_pgid_file.display(),
        background = pid_file.display()
    );
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), script],
        env: BTreeMap::new(),
        json_key: None,
    };
    let env = TestEnv::default();

    let value = tokio::time::timeout(Duration::from_secs(3), run_secret_command(&cmd, &env))
        .await
        .expect("secret command should return after the leader exits")?;
    wait_for_pid(&shell_pid_file)
        .await
        .expect("shell pid file should be written");
    let pid = wait_for_pid(&pid_file)
        .await
        .expect("background pid file should be written");

    assert_eq!(value.expose_secret(), "ok");

    assert!(
        wait_for_process_termination(pid, PROCESS_TERMINATION_WAIT_TIMEOUT).await,
        "successful secret command should clean up orphaned background processes"
    );
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
#[tokio::test]
async fn secret_command_runner_cancellation_kills_child_process_group() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("secret-command-background.pid");
    let script = format!(
        "sleep 30 </dev/null >/dev/null 2>&1 & echo $! > '{}'; wait",
        pid_file.display()
    );
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), script],
        env: BTreeMap::new(),
        json_key: None,
    };
    let env = TestEnv::default();

    let handle = tokio::spawn(async move {
        let _ = run_secret_command(&cmd, &env).await;
    });

    let pid = wait_for_pid(&pid_file)
        .await
        .expect("background pid file should be written");

    handle.abort();
    let _ = handle.await;

    assert!(
        wait_for_process_termination(pid, PROCESS_TERMINATION_WAIT_TIMEOUT).await,
        "secret command process group should be killed on cancellation"
    );
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
#[tokio::test]
async fn secret_command_runner_cancellation_kills_orphaned_process_group() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell_pid_file = dir.path().join("secret-command-shell.pid");
    let shell_pgid_file = dir.path().join("secret-command-shell.pgid");
    let bg_pid_file = dir.path().join("secret-command-background.pid");
    let script = format!(
        "echo $$ > '{shell}'; \
         awk '{{print $5}}' /proc/$$/stat > '{shell_pgid}'; \
         sleep 30 </dev/null >/dev/null 2>&1 & \
         bg=$!; \
         echo $bg > '{background}'; \
         while [ -r /proc/$bg/stat ]; do \
           bg_pgid=$(awk '{{print $5}}' /proc/$bg/stat 2>/dev/null || true); \
           shell_pgid=$(cat '{shell_pgid}' 2>/dev/null || true); \
           [ -n \"$shell_pgid\" ] && [ \"$bg_pgid\" = \"$shell_pgid\" ] && break; \
           sleep 0.01; \
         done; \
         exit 0",
        shell = shell_pid_file.display(),
        shell_pgid = shell_pgid_file.display(),
        background = bg_pid_file.display()
    );
    let cmd = SecretCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), script],
        env: BTreeMap::new(),
        json_key: None,
    };
    let env = TestEnv::default();

    let handle = tokio::spawn(async move {
        let _ = run_secret_command(&cmd, &env).await;
    });

    let shell_pid = wait_for_pid(&shell_pid_file)
        .await
        .expect("shell pid file should be written");
    let bg_pid = wait_for_pid(&bg_pid_file)
        .await
        .expect("background pid file should be written");

    assert!(
        wait_for_process_termination(shell_pid, PROCESS_TERMINATION_WAIT_TIMEOUT).await,
        "shell leader should exit before cancellation"
    );

    handle.abort();
    let _ = handle.await;

    assert!(
        wait_for_process_termination(bg_pid, PROCESS_TERMINATION_WAIT_TIMEOUT).await,
        "secret command cancellation should still kill orphaned background processes"
    );
    Ok(())
}
