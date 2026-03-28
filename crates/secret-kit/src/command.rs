#[cfg(test)]
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use omne_process_primitives::{
    CleanupDisposition, ProcessTreeCleanup, configure_command_for_process_tree,
};
use structured_text_kit::{CatalogText, StructuredText, StructuredTextScalarArg, structured_text};
use zeroize::Zeroize;

use crate::spec::SecretCommand;
use crate::{
    DEFAULT_SECRET_COMMAND_TIMEOUT_SECS, MAX_SECRET_COMMAND_OUTPUT_BYTES,
    MAX_SECRET_COMMAND_TIMEOUT_SECS, Result, SECRET_COMMAND_TIMEOUT_MS_ENV,
    SECRET_COMMAND_TIMEOUT_SECS_ENV, SecretBytes, SecretCommandRuntime, SecretError, SecretString,
    read_limited, secret_string_from_bytes,
};

struct SecretCommandChild {
    child: tokio::process::Child,
    cleanup: Option<ProcessTreeCleanup>,
}

impl SecretCommandChild {
    fn new(child: tokio::process::Child, program: &str) -> Result<Self> {
        let cleanup = ProcessTreeCleanup::new(&child).map_err(|err| {
            secret_command_error!(
                "error_detail.secret.command_cleanup_setup_failed",
                "program" => program,
                "error" => err.to_string()
            )
        })?;
        Ok(Self {
            child,
            cleanup: Some(cleanup),
        })
    }

    fn take_stdout(&mut self, program: &str) -> Result<tokio::process::ChildStdout> {
        self.child.stdout.take().ok_or_else(|| {
            secret_command_error!(
                "error_detail.secret.command_stdout_not_captured",
                "program" => program
            )
        })
    }

    fn take_stderr(&mut self, program: &str) -> Result<tokio::process::ChildStderr> {
        self.child.stderr.take().ok_or_else(|| {
            secret_command_error!(
                "error_detail.secret.command_stderr_not_captured",
                "program" => program
            )
        })
    }

    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    async fn kill(&mut self) -> std::io::Result<()> {
        if let Some(mut cleanup) = self.cleanup.take() {
            if cleanup.start_termination() == CleanupDisposition::TreeTerminationInitiated {
                return Ok(());
            }
            start_process_tree_cleanup(cleanup);
        }
        self.child.kill().await
    }

    fn kill_tree(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            start_process_tree_cleanup(cleanup);
        }
    }
}

impl Drop for SecretCommandChild {
    fn drop(&mut self) {
        if let Some(mut cleanup) = self.cleanup.take() {
            let _ = cleanup.start_termination();
            start_process_tree_cleanup(cleanup);
        }
    }
}

type CommandReadTask = tokio::task::JoinHandle<std::io::Result<(SecretBytes, bool)>>;

const TEXT_FILE_BUSY_RETRY_ATTEMPTS: usize = 5;
const TEXT_FILE_BUSY_RETRY_DELAY: Duration = Duration::from_millis(20);
#[cfg(all(unix, target_os = "linux"))]
const PROCESS_TREE_CLEANUP_RETRY_ATTEMPTS: usize = 50;
#[cfg(all(unix, target_os = "linux"))]
const PROCESS_TREE_CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(20);

fn start_process_tree_cleanup(cleanup: ProcessTreeCleanup) {
    #[cfg(all(unix, target_os = "linux"))]
    {
        std::thread::spawn(move || {
            cleanup.kill_tree();
            // Linux orphan cleanup relies on `/proc` to observe surviving process-group members
            // after the leader exits. That observation can lag well behind the caller's return on
            // slower CI runners, so continue retrying in the background without blocking success
            // or cancellation paths.
            for _ in 0..PROCESS_TREE_CLEANUP_RETRY_ATTEMPTS {
                std::thread::sleep(PROCESS_TREE_CLEANUP_RETRY_DELAY);
                cleanup.kill_tree();
            }
        });
    }

    #[cfg(not(all(unix, target_os = "linux")))]
    cleanup.kill_tree();
}

fn ensure_tokio_time_driver(program: &str) -> Result<()> {
    std::panic::catch_unwind(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    })
    .map_err(|_| {
        secret_command_error!(
            "error_detail.secret.command_runtime_missing_time_driver",
            "program" => program
        )
    })
}

#[derive(Clone, Copy, Debug)]
struct CommandStderrSummary {
    bytes: usize,
    hint: Option<&'static str>,
}

#[derive(Clone, Copy)]
enum CommandEnvSource {
    Ambient,
    Explicit,
}

struct CommandEnvSnapshot {
    ambient_pairs: Vec<(OsString, OsString)>,
    explicit_pairs: Vec<(OsString, OsString)>,
    timeout_ms: Option<OsString>,
    timeout_secs: Option<OsString>,
}

impl CommandEnvSnapshot {
    fn capture<E>(program: &str, env: &E) -> Self
    where
        E: SecretCommandRuntime + ?Sized,
    {
        let explicit_pairs = env.command_env_os_pairs().collect::<Vec<_>>();
        let timeout_ms = lookup_env_pair(explicit_pairs.as_slice(), SECRET_COMMAND_TIMEOUT_MS_ENV);
        let timeout_secs =
            lookup_env_pair(explicit_pairs.as_slice(), SECRET_COMMAND_TIMEOUT_SECS_ENV);

        Self {
            ambient_pairs: env.ambient_command_env_os_pairs(program).collect(),
            explicit_pairs,
            timeout_ms,
            timeout_secs,
        }
    }

    fn timeout(&self) -> Duration {
        let timeout_ms = self
            .timeout_ms
            .as_ref()
            .cloned()
            .and_then(parse_timeout_env_value)
            .filter(|value| *value > 0)
            .or_else(|| {
                self.timeout_secs
                    .as_ref()
                    .cloned()
                    .and_then(parse_timeout_env_value)
                    .filter(|value| *value > 0)
                    .map(|secs| secs.saturating_mul(1_000))
            })
            .unwrap_or(DEFAULT_SECRET_COMMAND_TIMEOUT_SECS.saturating_mul(1_000))
            .min(MAX_SECRET_COMMAND_TIMEOUT_SECS.saturating_mul(1_000));
        Duration::from_millis(timeout_ms)
    }
}

pub(crate) async fn run_secret_command<E>(cmd: &SecretCommand, env: &E) -> Result<SecretString>
where
    E: SecretCommandRuntime + ?Sized,
{
    ensure_tokio_time_driver(cmd.program.as_str())?;
    let snapshot = CommandEnvSnapshot::capture(cmd.program.as_str(), env);
    let resolved_program = resolve_command_program(cmd, env, &snapshot)?;
    let timeout = snapshot.timeout();
    let (mut child, stdout_task, stderr_task) =
        spawn_secret_command(cmd, resolved_program, snapshot).await?;
    let mut stdout_task = Some(stdout_task);
    let mut stderr_task = Some(stderr_task);
    let mut exit_status = None;
    let mut stdout = None;
    let mut stderr_done = false;
    let mut stderr_summary = None;
    let mut exited = false;
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        if exited && stdout.is_some() && stderr_done {
            break;
        }

        tokio::select! {
            wait_result = child.wait(), if !exited => {
                let wait_result = match wait_result {
                    Ok(status) => status,
                    Err(err) => {
                        let err = secret_command_error!(
                            "error_detail.secret.command_wait_failed",
                            "program" => cmd.program.as_str(),
                            "error" => err.to_string()
                        );
                        terminate_secret_command(&mut child, stdout_task.take(), stderr_task.take()).await;
                        return Err(err);
                    }
                };
                exit_status = Some(wait_result);
                exited = true;
                // The command leader may exit while helper processes from the same group are still
                // alive. Clean the process tree immediately on leader exit instead of waiting for
                // the tail `Drop` path, so success/cancellation paths with already-drained
                // stdout/stderr readers still reap orphaned descendants.
                child.kill_tree();
            }
            stdout_result = async {
                match stdout_task.as_mut() {
                    Some(task) => task.await,
                    None => unreachable!("stdout task should exist while selected"),
                }
            }, if stdout_task.is_some() => {
                stdout_task = None;
                let (bytes, truncated) = match join_command_output_task(cmd, "stdout", stdout_result) {
                    Ok(output) => output,
                    Err(err) => {
                        terminate_secret_command(&mut child, stdout_task.take(), stderr_task.take()).await;
                        return Err(err);
                    }
                };
                if truncated {
                    drop(bytes);
                    terminate_secret_command(&mut child, stdout_task.take(), stderr_task.take()).await;
                    return Err(command_output_too_large(cmd, "stdout"));
                }
                stdout = Some(bytes);
            }
            stderr_result = async {
                match stderr_task.as_mut() {
                    Some(task) => task.await,
                    None => unreachable!("stderr task should exist while selected"),
                }
            }, if stderr_task.is_some() => {
                stderr_task = None;
                let (stderr, truncated) = match join_command_output_task(cmd, "stderr", stderr_result) {
                    Ok(output) => output,
                    Err(err) => {
                        terminate_secret_command(&mut child, stdout_task.take(), stderr_task.take()).await;
                        return Err(err);
                    }
                };
                if truncated {
                    drop(stderr);
                    terminate_secret_command(&mut child, stdout_task.take(), stderr_task.take()).await;
                    return Err(command_output_too_large(cmd, "stderr"));
                }
                stderr_summary = CommandStderrSummary::from_bytes(&stderr);
                drop(stderr);
                stderr_done = true;
            }
            _ = &mut deadline => {
                terminate_secret_command(&mut child, stdout_task.take(), stderr_task.take()).await;
                return Err(secret_command_error!(
                    "error_detail.secret.command_timeout",
                    "program" => cmd.program.as_str(),
                    "timeout_ms" => timeout.as_millis().to_string()
                ));
            }
        }
    }

    let Some(status) = exit_status else {
        unreachable!("secret command loop should always observe the child exit status");
    };
    if let Err(err) = validate_command_status(cmd, status, stderr_summary.as_ref()) {
        drop(stdout);
        return Err(err);
    }

    let stdout = stdout.ok_or_else(|| {
        secret_command_error!(
            "error_detail.secret.command_output_read_failed",
            "program" => cmd.program.as_str(),
            "stream" => "stdout",
            "error" => "stdout not collected"
        )
    })?;

    decode_command_stdout(cmd, stdout)
}

#[cfg(test)]
pub(crate) fn secret_command_timeout_from_env<E>(env: &E) -> Duration
where
    E: SecretCommandRuntime + ?Sized,
{
    CommandEnvSnapshot::capture("", env).timeout()
}

async fn spawn_secret_command(
    cmd: &SecretCommand,
    resolved_program: String,
    snapshot: CommandEnvSnapshot,
) -> Result<(SecretCommandChild, CommandReadTask, CommandReadTask)> {
    let mut command = tokio::process::Command::new(resolved_program);
    command.env_clear();
    apply_command_env(&mut command, cmd.program.as_str(), snapshot);
    command.args(&cmd.args);
    for (key, value) in &cmd.env {
        command.env(key, value);
    }

    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    configure_command_for_process_tree(&mut command);

    let child = spawn_command_with_retry(&mut command)
        .await
        .map_err(|err| {
            secret_command_error!(
                "error_detail.secret.command_spawn_failed",
                "program" => cmd.program.as_str(),
                "error" => err.to_string()
            )
        })?;
    let mut child = SecretCommandChild::new(child, cmd.program.as_str())?;
    let stdout = child.take_stdout(cmd.program.as_str())?;
    let stderr = child.take_stderr(cmd.program.as_str())?;
    let stdout_task = tokio::spawn(read_limited(stdout, MAX_SECRET_COMMAND_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(read_limited(stderr, MAX_SECRET_COMMAND_OUTPUT_BYTES));
    Ok((child, stdout_task, stderr_task))
}

async fn spawn_command_with_retry(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    retry_text_file_busy(
        TEXT_FILE_BUSY_RETRY_ATTEMPTS,
        TEXT_FILE_BUSY_RETRY_DELAY,
        || command.spawn(),
    )
    .await
}

async fn retry_text_file_busy<T, F>(
    attempts: usize,
    delay: Duration,
    mut operation: F,
) -> std::io::Result<T>
where
    F: FnMut() -> std::io::Result<T>,
{
    for attempt in 0..=attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(err) if should_retry_text_file_busy(&err) && attempt < attempts => {
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err),
        }
    }

    unreachable!("retry loop should always return or error");
}

fn should_retry_text_file_busy(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(26)
    }

    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[tokio::test]
    async fn retry_text_file_busy_retries_until_success() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);

        let value = retry_text_file_busy(2, Duration::ZERO, || {
            let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if attempt < 2 {
                return Err(std::io::Error::from_raw_os_error(26));
            }
            Ok("ok")
        })
        .await
        .expect("third attempt succeeds");

        assert_eq!(value, "ok");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn retry_text_file_busy_returns_non_retryable_error_immediately() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);

        let err = retry_text_file_busy(5, Duration::ZERO, || {
            attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err::<(), _>(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            ))
        })
        .await
        .expect_err("non-retryable error should be returned");

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}

fn apply_command_env(
    command: &mut tokio::process::Command,
    program: &str,
    snapshot: CommandEnvSnapshot,
) {
    let provider = SecretCliProgram::from_program(program);
    append_command_env(
        command,
        provider,
        CommandEnvSource::Ambient,
        snapshot.ambient_pairs.into_iter(),
    );
    append_command_env(
        command,
        provider,
        CommandEnvSource::Explicit,
        snapshot.explicit_pairs.into_iter(),
    );
}

fn resolve_command_program<E>(
    cmd: &SecretCommand,
    env: &E,
    snapshot: &CommandEnvSnapshot,
) -> Result<String>
where
    E: SecretCommandRuntime + ?Sized,
{
    if let Some(program) = env.resolve_command_program(cmd.program.as_str()) {
        validate_command_program_override(cmd.program.as_str(), program.as_str())?;
        return Ok(program);
    }

    let Some(provider) = SecretCliProgram::from_program(cmd.program.as_str()) else {
        return Ok(cmd.program.clone());
    };

    resolve_builtin_program(snapshot, provider).ok_or_else(|| {
        secret_command_error!(
            "error_detail.secret.command_spawn_failed",
            "program" => cmd.program.as_str(),
            "error" => format!("{} not found on ambient PATH", cmd.program.as_str())
        )
    })
}

fn append_command_env(
    command: &mut tokio::process::Command,
    provider: Option<SecretCliProgram>,
    source: CommandEnvSource,
    pairs: impl Iterator<Item = (OsString, OsString)>,
) {
    for (key, value) in pairs {
        let allowed = match provider {
            Some(provider) => key
                .to_str()
                .is_some_and(|key| is_allowed_command_env_var(provider, source, key)),
            None => true,
        };
        if !allowed {
            best_effort_zeroize_os_string(key);
            best_effort_zeroize_os_string(value);
            continue;
        }
        command.env(&key, &value);
        best_effort_zeroize_os_string(key);
        best_effort_zeroize_os_string(value);
    }
}

async fn terminate_secret_command(
    child: &mut SecretCommandChild,
    stdout_task: Option<CommandReadTask>,
    stderr_task: Option<CommandReadTask>,
) {
    let _ = child.kill().await;
    let _ = child.wait().await;
    cancel_command_output_tasks(stdout_task, stderr_task).await;
}

async fn cancel_command_output_tasks(
    stdout_task: Option<CommandReadTask>,
    stderr_task: Option<CommandReadTask>,
) {
    cancel_command_output_task(stdout_task).await;
    cancel_command_output_task(stderr_task).await;
}

async fn cancel_command_output_task(task: Option<CommandReadTask>) {
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }
}

fn join_command_output_task(
    cmd: &SecretCommand,
    stream: &str,
    result: std::result::Result<std::io::Result<(SecretBytes, bool)>, tokio::task::JoinError>,
) -> Result<(SecretBytes, bool)> {
    result
        .map_err(|err| {
            secret_command_error!(
                "error_detail.secret.command_reader_join_failed",
                "stream" => stream,
                "error" => err.to_string()
            )
        })?
        .map_err(|err| {
            secret_command_error!(
                "error_detail.secret.command_output_read_failed",
                "program" => cmd.program.as_str(),
                "stream" => stream,
                "error" => err.to_string()
            )
        })
}

fn command_output_too_large(cmd: &SecretCommand, stream: &str) -> SecretError {
    match stream {
        "stdout" => secret_command_error!(
            "error_detail.secret.command_stdout_too_large",
            "program" => cmd.program.as_str(),
            "max_bytes" => MAX_SECRET_COMMAND_OUTPUT_BYTES.to_string()
        ),
        "stderr" => secret_command_error!(
            "error_detail.secret.command_stderr_too_large",
            "program" => cmd.program.as_str(),
            "max_bytes" => MAX_SECRET_COMMAND_OUTPUT_BYTES.to_string()
        ),
        _ => unreachable!("unexpected secret command stream"),
    }
}

fn validate_command_status(
    cmd: &SecretCommand,
    status: std::process::ExitStatus,
    stderr_summary: Option<&CommandStderrSummary>,
) -> Result<()> {
    if status.success() {
        return Ok(());
    }

    Err(command_failed_status_error(cmd, status, stderr_summary))
}

fn decode_command_stdout(cmd: &SecretCommand, stdout: SecretBytes) -> Result<SecretString> {
    secret_string_from_bytes(stdout, |_| {
        secret_command_error!(
            "error_detail.secret.command_stdout_not_utf8",
            "program" => cmd.program.as_str()
        )
    })
}

#[cfg(test)]
pub(crate) fn build_command_env(
    program: &str,
    command_env: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let Some(provider) = SecretCliProgram::from_program(program) else {
        return command_env;
    };

    command_env
        .into_iter()
        .filter(|(key, _)| is_allowed_command_env_var(provider, CommandEnvSource::Explicit, key))
        .collect()
}

fn parse_timeout_env_value(raw: OsString) -> Option<u64> {
    raw.into_string().ok()?.trim().parse::<u64>().ok()
}

fn lookup_env_pair(pairs: &[(OsString, OsString)], key: &str) -> Option<OsString> {
    pairs.iter().find_map(|(candidate, value)| {
        candidate
            .to_str()
            .is_some_and(|candidate| env_var_name_matches(candidate, key))
            .then(|| value.clone())
    })
}

#[derive(Clone, Copy)]
enum SecretCliProgram {
    Vault,
    Aws,
    Gcloud,
    Az,
}

impl SecretCliProgram {
    fn from_program(program: &str) -> Option<Self> {
        match Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
        {
            Some("vault") => Some(Self::Vault),
            Some("aws") => Some(Self::Aws),
            Some("gcloud") => Some(Self::Gcloud),
            Some("az") => Some(Self::Az),
            _ => None,
        }
    }

    fn program_name(self) -> &'static str {
        match self {
            Self::Vault => "vault",
            Self::Aws => "aws",
            Self::Gcloud => "gcloud",
            Self::Az => "az",
        }
    }
}

fn validate_command_program_override(program: &str, resolved_program: &str) -> Result<()> {
    let Some(provider) = SecretCliProgram::from_program(program) else {
        return Ok(());
    };
    let override_path = Path::new(resolved_program);
    if !override_path.is_absolute() {
        return Err(secret_command_error!(
            "error_detail.secret.command_program_override_not_absolute",
            "program" => program
        ));
    }
    if command_override_matches_provider(provider, override_path) {
        return Ok(());
    }
    Err(secret_command_error!(
        "error_detail.secret.command_program_override_invalid_name",
        "program" => program,
        "resolved_program" => resolved_program
    ))
}

fn command_override_matches_provider(provider: SecretCliProgram, override_path: &Path) -> bool {
    let expected = provider.program_name();
    let matches_basename = override_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| program_name_matches(name, expected));

    #[cfg(windows)]
    let matches_stem = override_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| program_name_matches(stem, expected));

    #[cfg(not(windows))]
    let matches_stem = false;

    matches_basename || matches_stem
}

#[cfg(windows)]
fn program_name_matches(value: &str, expected: &str) -> bool {
    value.eq_ignore_ascii_case(expected)
}

#[cfg(not(windows))]
fn program_name_matches(value: &str, expected: &str) -> bool {
    value == expected
}

pub(crate) fn filtered_ambient_command_env_pairs(
    program: &str,
) -> Box<dyn Iterator<Item = (String, String)>> {
    Box::new(
        filtered_ambient_command_env_os_pairs(program)
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?))),
    )
}

pub(crate) fn filtered_ambient_command_env_os_pairs(
    program: &str,
) -> Box<dyn Iterator<Item = (OsString, OsString)>> {
    let provider = SecretCliProgram::from_program(program);
    Box::new(std::env::vars_os().filter_map(move |(key, value)| {
        let provider = provider?;
        let allowed = key.to_str().is_some_and(|key| {
            is_allowed_command_env_var(provider, CommandEnvSource::Ambient, key)
        });
        allowed.then_some((key, value))
    }))
}

fn is_allowed_command_env_var(
    program: SecretCliProgram,
    source: CommandEnvSource,
    key: &str,
) -> bool {
    if matches!(source, CommandEnvSource::Explicit) && is_command_search_path_env_var(key) {
        return false;
    }

    const COMMON_ALLOWED: &[&str] = &[
        "PATH",
        "Path",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramData",
        "PROGRAMDATA",
        "SystemRoot",
        "SYSTEMROOT",
        "ComSpec",
        "COMSPEC",
        "PATHEXT",
        "windir",
        "WINDIR",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LC_MESSAGES",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "all_proxy",
    ];

    if COMMON_ALLOWED
        .iter()
        .any(|candidate| env_var_name_matches(key, candidate))
    {
        return true;
    }

    match program {
        SecretCliProgram::Vault => env_var_has_prefix(key, "VAULT_"),
        SecretCliProgram::Aws => env_var_has_prefix(key, "AWS_"),
        SecretCliProgram::Gcloud => {
            env_var_has_prefix(key, "CLOUDSDK_")
                || env_var_has_prefix(key, "GOOGLE_")
                || env_var_has_prefix(key, "BOTO_")
        }
        SecretCliProgram::Az => {
            env_var_has_prefix(key, "AZURE_")
                || env_var_has_prefix(key, "IDENTITY_")
                || env_var_has_prefix(key, "MSI_")
        }
    }
}

fn is_command_search_path_env_var(key: &str) -> bool {
    ["PATH", "Path"]
        .iter()
        .any(|candidate| env_var_name_matches(key, candidate))
}

#[cfg(not(windows))]
fn resolve_builtin_program(
    snapshot: &CommandEnvSnapshot,
    program: SecretCliProgram,
) -> Option<String> {
    let path = lookup_env_pair(snapshot.ambient_pairs.as_slice(), "PATH")?;
    resolve_program_on_path(program.program_name(), path.as_os_str())
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn resolve_builtin_program(
    snapshot: &CommandEnvSnapshot,
    program: SecretCliProgram,
) -> Option<String> {
    let path = lookup_env_pair(snapshot.ambient_pairs.as_slice(), "PATH")?;
    let path_extensions = command_path_extensions(
        lookup_env_pair(snapshot.ambient_pairs.as_slice(), "PATHEXT").as_deref(),
    );
    resolve_program_on_path_with_extensions(
        program.program_name(),
        path.as_os_str(),
        path_extensions.as_slice(),
    )
    .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
fn resolve_program_on_path(program: &str, path: &OsStr) -> Option<PathBuf> {
    for directory in std::env::split_paths(path) {
        let Some(directory) = trusted_command_search_directory(directory) else {
            continue;
        };
        let candidates = [OsString::from(program)];

        for candidate_name in candidates {
            let candidate = directory.join(&candidate_name);
            if is_launchable_program_path(candidate.as_path()) {
                return Some(candidate);
            }
        }
    }
    None
}

fn trusted_command_search_directory(directory: PathBuf) -> Option<PathBuf> {
    directory.is_absolute().then_some(directory)
}

#[cfg(windows)]
fn resolve_program_on_path_with_extensions(
    program: &str,
    path: &OsStr,
    path_extensions: &[String],
) -> Option<PathBuf> {
    for directory in std::env::split_paths(path) {
        let Some(directory) = trusted_command_search_directory(directory) else {
            continue;
        };
        let candidates = command_path_candidates(program, path_extensions);

        for candidate_name in candidates {
            let candidate = directory.join(&candidate_name);
            if is_launchable_program_path(candidate.as_path()) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn command_path_candidates(program: &str, path_extensions: &[String]) -> Vec<OsString> {
    if Path::new(program).extension().is_some() {
        return vec![OsString::from(program)];
    }

    let mut candidates = vec![OsString::from(program)];
    for extension in path_extensions {
        candidates.push(OsString::from(format!("{program}{extension}")));
    }
    candidates
}

#[cfg(windows)]
fn command_path_extensions(path_ext: Option<&OsStr>) -> Vec<String> {
    const DEFAULT_EXTENSIONS: &str = ".COM;.EXE;.BAT;.CMD";

    path_ext
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| DEFAULT_EXTENSIONS.to_string())
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            if extension.starts_with('.') {
                extension.to_string()
            } else {
                format!(".{extension}")
            }
        })
        .collect()
}

#[cfg(not(windows))]
fn is_launchable_program_path(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0))
}

#[cfg(windows)]
fn is_launchable_program_path(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
pub(crate) fn resolve_program_on_path_for_test(program: &str, path: &OsStr) -> Option<String> {
    resolve_program_on_path(program, path).map(|path| path.to_string_lossy().into_owned())
}

#[cfg(all(test, windows))]
pub(crate) fn resolve_program_on_path_with_extensions_for_test(
    program: &str,
    path: &OsStr,
    path_ext: Option<&OsStr>,
) -> Option<String> {
    let path_extensions = command_path_extensions(path_ext);
    resolve_program_on_path_with_extensions(program, path, path_extensions.as_slice())
        .map(|path| path.to_string_lossy().into_owned())
}

fn best_effort_zeroize_os_string(value: OsString) {
    let mut bytes = value.into_encoded_bytes();
    bytes.zeroize();
}

impl CommandStderrSummary {
    fn from_bytes(stderr: &SecretBytes) -> Option<Self> {
        let bytes = stderr.as_ref();
        (!bytes.is_empty()).then_some(Self {
            bytes: bytes.len(),
            hint: classify_command_stderr(bytes),
        })
    }
}

fn command_failed_status_error(
    cmd: &SecretCommand,
    status: std::process::ExitStatus,
    stderr_summary: Option<&CommandStderrSummary>,
) -> SecretError {
    let mut text = match CatalogText::try_new("error_detail.secret.command_failed_status") {
        Ok(text) => text,
        Err(_) => unreachable!("literal command error code should always validate"),
    };
    push_command_error_arg(&mut text, "program", cmd.program.as_str());
    push_command_error_arg(&mut text, "status", status.to_string());

    if let Some(stderr_summary) = stderr_summary {
        push_command_error_arg(&mut text, "stderr_bytes", stderr_summary.bytes as u64);
        if let Some(hint) = stderr_summary.hint {
            push_command_error_arg(&mut text, "stderr_hint", hint);
        }
    }

    SecretError::Command(StructuredText::from(text))
}

fn push_command_error_arg<V>(text: &mut CatalogText, name: &'static str, value: V)
where
    V: StructuredTextScalarArg,
{
    if text.try_with_value_arg(name, value).is_err() {
        unreachable!("literal command error args should always validate");
    }
}

pub(crate) fn classify_command_stderr(stderr: &[u8]) -> Option<&'static str> {
    // This is intentionally a coarse diagnostic hint. It must stay safe to log and should never
    // become a branch point for application behavior.
    const AUTH_PATTERNS: &[&[u8]] = &[
        b"not logged in",
        b"login",
        b"unauthorized",
        b"unauthenticated",
        b"forbidden",
        b"access denied",
        b"permission denied",
        b"expiredtoken",
        b"invalidclienttokenid",
        b"insufficient authentication",
    ];
    const NOT_FOUND_PATTERNS: &[&[u8]] = &[
        b"not found",
        b"no such secret",
        b"resource not found",
        b"does not exist",
    ];
    const NETWORK_PATTERNS: &[&[u8]] = &[
        b"could not resolve",
        b"no such host",
        b"name resolution",
        b"connection refused",
        b"connection reset",
        b"network is unreachable",
    ];
    const TIMEOUT_PATTERNS: &[&[u8]] = &[
        b"timed out",
        b"timeout",
        b"deadline exceeded",
        b"context deadline exceeded",
    ];
    const RATE_LIMIT_PATTERNS: &[&[u8]] = &[b"rate exceeded", b"too many requests", b"throttl"];

    if stderr_matches_any(stderr, AUTH_PATTERNS) {
        return Some("auth");
    }
    if stderr_matches_any(stderr, NOT_FOUND_PATTERNS) {
        return Some("not_found");
    }
    if stderr_matches_any(stderr, NETWORK_PATTERNS) {
        return Some("network");
    }
    if stderr_matches_any(stderr, TIMEOUT_PATTERNS) {
        return Some("timeout");
    }
    if stderr_matches_any(stderr, RATE_LIMIT_PATTERNS) {
        return Some("rate_limit");
    }
    None
}

fn stderr_matches_any(stderr: &[u8], patterns: &[&[u8]]) -> bool {
    patterns
        .iter()
        .copied()
        .any(|pattern| stderr_contains_ascii_case_insensitive(stderr, pattern))
}

fn stderr_contains_ascii_case_insensitive(stderr: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }

    stderr
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(windows)]
fn env_var_name_matches(value: &str, expected: &str) -> bool {
    value.eq_ignore_ascii_case(expected)
}

#[cfg(not(windows))]
fn env_var_name_matches(value: &str, expected: &str) -> bool {
    value == expected
}

#[cfg(windows)]
fn env_var_has_prefix(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

#[cfg(not(windows))]
fn env_var_has_prefix(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
}
