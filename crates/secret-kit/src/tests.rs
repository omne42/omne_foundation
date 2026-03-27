use std::borrow::Cow;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::command::{
    build_command_env, resolve_program_on_path_for_test, run_secret_command,
    secret_command_timeout_from_env,
};
use crate::json::extract_json_key;
use crate::spec::{
    SecretCommand, build_secret_command, prepare_default_secret_resolution,
    resolve_prepared_default_secret,
};
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
    vars: BTreeMap<String, String>,
    command_vars: BTreeMap<String, String>,
    command_programs: BTreeMap<String, String>,
}

impl Default for TestEnv {
    fn default() -> Self {
        Self {
            cache_partition: "default-test-env".to_string(),
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

trait TestSecretResolverExt: SecretResolver {
    async fn resolve_secret_text(&self, spec: &str, env: &dyn SecretEnvironment) -> Result<String> {
        self.resolve_secret(spec, SecretResolutionContext::ambient(env))
            .await
            .map(|secret| secret.expose_secret().to_owned())
    }
}

impl<T> TestSecretResolverExt for T where T: SecretResolver + ?Sized {}

fn test_cache_scope(spec: &str) -> Option<String> {
    match SecretSpec::parse(spec).ok()? {
        SecretSpec::File { path } if Path::new(&path).is_absolute() => {
            let metadata = std::fs::metadata(&path).ok()?;
            Some(format!("test-file:{path}:{}", metadata.len()))
        }
        _ => None,
    }
}

#[derive(Default)]
struct CountingResolver {
    calls: AtomicUsize,
}

impl SecretResolver for CountingResolver {
    async fn resolve_secret(
        &self,
        _spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(SecretString::from(format!("value-{call}")))
    }
}

impl CacheAwareSecretResolver for CountingResolver {
    type Prepared = ();

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
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
        self.resolve_secret("", context).await
    }
}

#[derive(Default)]
struct RetryResolver {
    calls: AtomicUsize,
}

impl SecretResolver for RetryResolver {
    async fn resolve_secret(
        &self,
        _spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 1 {
            return Err(invalid_response!("error_detail.secret.not_resolvable"));
        }
        Ok(SecretString::from("recovered"))
    }
}

impl CacheAwareSecretResolver for RetryResolver {
    type Prepared = ();

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
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
        self.resolve_secret("", context).await
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
    async fn resolve_secret(
        &self,
        _spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::time::sleep(self.delay).await;
        Ok(SecretString::from(format!("value-{call}")))
    }
}

impl CacheAwareSecretResolver for SlowResolver {
    type Prepared = ();

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
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
        self.resolve_secret("", context).await
    }
}

#[derive(Default)]
struct MismatchedHintResolver {
    calls: AtomicUsize,
}

impl SecretResolver for MismatchedHintResolver {
    async fn resolve_secret(
        &self,
        spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        Ok(SecretString::from(format!(
            "{spec}-{}",
            self.calls.fetch_add(1, Ordering::SeqCst) + 1
        )))
    }
}

impl CacheAwareSecretResolver for MismatchedHintResolver {
    type Prepared = String;

    fn lookup_secret_cache_scope(
        &self,
        _spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some("shared-hint".to_string()))
    }

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(PreparedSecretResolution::cached(
            spec.to_string(),
            format!("prepared:{spec}"),
        ))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret(prepared.as_str(), context).await
    }
}

#[derive(Default)]
struct EnvironmentScopedResolver {
    calls: AtomicUsize,
}

impl SecretResolver for EnvironmentScopedResolver {
    async fn resolve_secret(
        &self,
        spec: &str,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(context
            .environment()
            .get_secret(spec)
            .expect("test env secret should exist for environment-scoped cache test"))
    }
}

impl CacheAwareSecretResolver for EnvironmentScopedResolver {
    type Prepared = String;

    fn lookup_secret_cache_scope(
        &self,
        spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<Option<String>> {
        Ok(Some(spec.to_string()))
    }

    async fn prepare_secret_resolution(
        &self,
        spec: &str,
        _context: SecretResolutionContext<'_>,
    ) -> Result<PreparedSecretResolution<Self::Prepared>> {
        Ok(PreparedSecretResolution::cached(
            spec.to_string(),
            spec.to_string(),
        ))
    }

    async fn resolve_prepared_secret(
        &self,
        prepared: Self::Prepared,
        context: SecretResolutionContext<'_>,
    ) -> Result<SecretString> {
        self.resolve_secret(prepared.as_str(), context).await
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
fn process_terminated_or_zombie(pid: u32) -> bool {
    let status_path = format!("/proc/{pid}/status");
    match std::fs::read_to_string(status_path) {
        Ok(status) => status
            .lines()
            .find(|line| line.starts_with("State:"))
            .map(|line| line.contains("\tZ") || line.contains(" zombie"))
            .unwrap_or(false),
        Err(err) => err.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(all(unix, target_os = "linux"))]
async fn wait_for_pid(path: &std::path::Path) -> Option<u32> {
    for _ in 0..100 {
        if let Ok(raw) = tokio::fs::read_to_string(path).await
            && let Ok(pid) = raw.trim().parse::<u32>()
        {
            return Some(pid);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

#[cfg(all(unix, target_os = "linux"))]
async fn wait_for_process_termination(pid: u32, attempts: usize) -> bool {
    for _ in 0..attempts {
        if process_terminated_or_zombie(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
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

    assert_eq!(std::path::Path::new(&resolved), path.as_path());
    assert!(std::path::Path::new(&resolved).is_absolute());
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

    assert_eq!(std::path::Path::new(&resolved), absolute_program.as_path());
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

    assert_eq!(std::path::Path::new(&resolved), executable.as_path());
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
    assert_eq!(std::path::Path::new(&resolved), path.as_path());

    let missing = crate::command::resolve_program_on_path_with_extensions_for_test(
        "vault",
        dir.path().as_os_str(),
        Some(OsStr::new(".EXE")),
    );
    assert_eq!(missing, None);
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_secret_uses_ambient_snapshot_path_for_builtin_resolution() -> Result<()> {
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

    tokio::time::sleep(Duration::from_millis(10)).await;
    let value = resolve_secret_text("secret://vault/secret/demo?field=token", &env).await?;
    assert_eq!(value, "resolved-from-snapshot");
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

    let first = resolver.resolve_secret_text("spec-a", &env).await?;
    let second = resolver.resolve_secret_text("spec-a", &env).await?;
    let third = resolver.resolve_secret_text("spec-b", &env).await?;

    assert_eq!(first, "spec-a-1");
    assert_eq!(second, "spec-a-1");
    assert_eq!(third, "spec-b-2");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 2);
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

    let first = resolver.resolve_secret_text("API_KEY", &env_a).await?;
    let second = resolver.resolve_secret_text("API_KEY", &env_b).await?;
    let third = resolver.resolve_secret_text("API_KEY", &env_a).await?;

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

    let first = resolver.resolve_secret_text("API_KEY", &env_a).await?;
    let second = resolver.resolve_secret_text("API_KEY", &env_b).await?;

    assert_eq!(first, "prod");
    assert_eq!(second, "prod");
    assert_eq!(resolver.inner().calls.load(Ordering::SeqCst), 1);
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
        async fn resolve_secret(
            &self,
            _spec: &str,
            _context: SecretResolutionContext<'_>,
        ) -> Result<SecretString> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(SecretString::from(format!("value-{call}")))
        }
    }

    impl CacheAwareSecretResolver for EmptyScopeResolver {
        type Prepared = ();

        async fn prepare_secret_resolution(
            &self,
            _spec: &str,
            _context: SecretResolutionContext<'_>,
        ) -> Result<PreparedSecretResolution<Self::Prepared>> {
            Ok(PreparedSecretResolution::cached((), ""))
        }

        async fn resolve_prepared_secret(
            &self,
            _prepared: Self::Prepared,
            context: SecretResolutionContext<'_>,
        ) -> Result<SecretString> {
            self.resolve_secret("", context).await
        }
    }

    let resolver =
        CachingSecretResolver::new(EmptyScopeResolver::default(), Duration::from_secs(60));
    let env = TestEnv::default();

    let first = resolver.resolve_secret_text("empty-scope", &env).await?;
    let second = resolver.resolve_secret_text("empty-scope", &env).await?;

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

    let prepared = prepare_default_secret_resolution(&spec).await?;

    assert!(prepared.cache_scope().is_none());
    Ok(())
}

#[tokio::test]
async fn default_prepared_resolution_reads_file_at_resolve_time() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.txt");
    tokio::fs::write(&path, b"v1").await?;
    let spec = format!("secret://file?path={}", path.to_string_lossy());

    let prepared = prepare_default_secret_resolution(&spec).await?;
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
    let prepared = prepare_default_secret_resolution("secret://env/TEST_SECRET").await?;

    assert!(prepared.cache_scope().is_none());
    Ok(())
}

#[tokio::test]
async fn default_secret_resolver_does_not_hint_file_cache_scope() -> Result<()> {
    let (_dir, spec) = temp_file_spec("cached.txt", b"cached").await?;
    let env = TestEnv::default();
    let hint = DefaultSecretResolver
        .lookup_secret_cache_scope(&spec, SecretResolutionContext::new(&env, &env))?;

    assert_eq!(hint, None);
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
    let pid_file = dir.path().join("secret-command-background.pid");
    let script = format!("sleep 30 & echo $! > '{}'; printf ok", pid_file.display());
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
    let pid = wait_for_pid(&pid_file)
        .await
        .expect("background pid file should be written");

    assert_eq!(value.expose_secret(), "ok");

    assert!(
        wait_for_process_termination(pid, 500).await,
        "successful secret command should clean up orphaned background processes"
    );
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
#[tokio::test]
async fn secret_command_runner_cancellation_kills_child_process_group() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("secret-command-background.pid");
    let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());
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
        wait_for_process_termination(pid, 500).await,
        "secret command process group should be killed on cancellation"
    );
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
#[tokio::test]
async fn secret_command_runner_cancellation_kills_orphaned_process_group() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell_pid_file = dir.path().join("secret-command-shell.pid");
    let bg_pid_file = dir.path().join("secret-command-background.pid");
    let script = format!(
        "echo $$ > '{shell}'; sleep 30 & echo $! > '{background}'; exit 0",
        shell = shell_pid_file.display(),
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
        wait_for_process_termination(shell_pid, 500).await,
        "shell leader should exit before cancellation"
    );

    handle.abort();
    let _ = handle.await;

    assert!(
        wait_for_process_termination(bg_pid, 500).await,
        "secret command cancellation should still kill orphaned background processes"
    );
    Ok(())
}
