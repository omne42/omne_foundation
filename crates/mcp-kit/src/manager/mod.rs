//! Connection cache + MCP initialize + request helpers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::Child;

use crate::error::{ErrorKind, tagged_message, wrap_kind};
use crate::{
    Config, MCP_PROTOCOL_VERSION, Root, ServerConfig, ServerName, Session, TrustMode,
    UntrustedStreamableHttpPolicy,
};

mod connect;
mod convenience;
mod handlers;
mod lifecycle;
mod path_identity;
mod placeholders;
mod streamable_http_validation;

pub use handlers::{
    ServerNotificationContext, ServerNotificationHandler, ServerRequestContext,
    ServerRequestHandler, ServerRequestOutcome,
};

#[cfg(test)]
use connect::{absolutize_with_base, stdout_log_path_within_root};

#[cfg(test)]
use placeholders::expand_placeholders_trusted;

#[cfg(test)]
use streamable_http_validation::validate_streamable_http_url_untrusted;

#[cfg(test)]
use streamable_http_validation::validate_streamable_http_url_untrusted_dns;

pub(crate) use connect::{
    ConnectContext, ConnectionServerConfigIdentity, connect_transport,
    effective_server_config_identity, raw_server_config_identity,
};
pub(crate) use handlers::{current_manager_handler_scope_token, is_in_manager_handler_scope};
pub(crate) use path_identity::{resolve_connection_cwd, resolve_connection_cwd_with_base};
pub(crate) use streamable_http_validation::should_disconnect_after_jsonrpc_error;

static NEXT_MANAGER_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONNECTION_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

macro_rules! config_bail {
    ($($arg:tt)*) => {
        return Err(tagged_message(ErrorKind::Config, format!($($arg)*)).into())
    };
}

macro_rules! manager_state_bail {
    ($($arg:tt)*) => {
        return Err(tagged_message(ErrorKind::ManagerState, format!($($arg)*)).into())
    };
}

macro_rules! timeout_bail {
    ($($arg:tt)*) => {
        return Err(tagged_message(ErrorKind::Timeout, format!($($arg)*)).into())
    };
}

fn next_connection_id() -> u64 {
    NEXT_CONNECTION_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)
}

fn parse_server_name_anyhow(server_name: &str) -> anyhow::Result<ServerName> {
    ServerName::parse(server_name)
        .with_context(|| format!("invalid mcp server name {server_name:?}"))
}

fn normalize_server_name_lookup(server_name: &str) -> &str {
    server_name.trim()
}

fn duplicate_live_connection_error(server_name: &ServerName, target: &str) -> anyhow::Error {
    tagged_message(
        ErrorKind::ManagerState,
        format!(
            "mcp server {server_name} is already connected; refusing to drop an unused {target} (disconnect first)"
        ),
    )
}

fn reject_duplicate_custom_jsonrpc_client(
    server_name: &ServerName,
    client: &mut mcp_jsonrpc::Client,
) -> anyhow::Error {
    client.close_in_background_once(format!(
        "duplicate custom JSON-RPC attach rejected for already-connected server {server_name}"
    ));
    if let Some(child) = client.take_child() {
        lifecycle::reap_stale_child_best_effort(child);
    }
    duplicate_live_connection_error(server_name, "custom JSON-RPC client")
}

pub(crate) fn contains_wait_timeout(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<mcp_jsonrpc::Error>()
            .is_some_and(mcp_jsonrpc::Error::is_wait_timeout)
    })
}

pub(crate) fn ensure_tokio_time_driver(operation: &'static str) -> anyhow::Result<()> {
    std::panic::catch_unwind(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    })
    .map_err(|_| {
        anyhow::anyhow!(
            "tokio runtime time driver is not enabled; build the runtime with enable_time() ({operation})"
        )
    })
}

pub(crate) fn resolve_config_connection_cwd(
    config_root: Option<&Path>,
    cwd: &Path,
) -> anyhow::Result<PathBuf> {
    if cwd.is_relative() && config_root.is_none() {
        return Err(tagged_message(
            ErrorKind::Config,
            "relative MCP cwd requires a loaded config path/thread root; pass an absolute cwd or load mcp.json from disk",
        ));
    }

    resolve_connection_cwd_with_base(config_root, cwd)
}

fn validate_protocol_version(protocol_version: impl Into<String>) -> crate::Result<String> {
    let protocol_version = protocol_version.into();
    if protocol_version.trim().is_empty() {
        return Err(
            tagged_message(ErrorKind::Config, "mcp protocol version must not be empty").into(),
        );
    }
    Ok(protocol_version)
}

fn validate_capabilities(capabilities: Value) -> crate::Result<Value> {
    if !capabilities.is_object() {
        return Err(tagged_message(
            ErrorKind::Config,
            "mcp client capabilities must be a JSON object",
        )
        .into());
    }
    Ok(capabilities)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtocolVersionCheck {
    /// Fail closed (default): require a string `initialize.result.protocolVersion` and reject
    /// mismatches.
    #[default]
    Strict,
    /// Allow mismatches but record them in `Manager::protocol_version_mismatches`.
    Warn,
    /// Allow mismatches without recording.
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolVersionMismatch {
    pub server_name: ServerName,
    pub client_protocol_version: String,
    pub server_protocol_version: String,
}

#[derive(Debug, Default)]
struct ServerHandlerTimeoutCounts {
    counters: Mutex<HashMap<ServerName, Arc<AtomicU64>>>,
}

impl ServerHandlerTimeoutCounts {
    fn counter_for(&self, server_name: &ServerName) -> Arc<AtomicU64> {
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counters
            .entry(server_name.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone()
    }

    fn count(&self, server_name: &str) -> u64 {
        self.counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(server_name)
            .map_or(0, |counter| counter.load(Ordering::Relaxed))
    }

    fn snapshot(&self) -> HashMap<ServerName, u64> {
        let counters = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshot = HashMap::with_capacity(counters.len());
        for (name, counter) in counters.iter() {
            snapshot.insert(name.clone(), counter.load(Ordering::Relaxed));
        }
        snapshot
    }

    fn take_and_reset(&self) -> HashMap<ServerName, u64> {
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshot = HashMap::with_capacity(counters.len());
        for (name, counter) in counters.iter() {
            snapshot.insert(name.clone(), counter.swap(0, Ordering::Relaxed));
        }
        // Compact stale zeroed entries that are no longer shared with active handler tasks.
        counters.retain(|_, counter| {
            counter.load(Ordering::Relaxed) > 0 || Arc::strong_count(counter) > 1
        });
        snapshot
    }

    fn remove(&self, server_name: &str) {
        self.counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(server_name);
    }
}

pub struct Manager {
    instance_id: u64,
    active_handler_scopes: Arc<AtomicU64>,
    conns: HashMap<ServerName, Connection>,
    connection_cwds: HashMap<ServerName, PathBuf>,
    connection_server_configs: HashMap<ServerName, ConnectionServerConfigIdentity>,
    init_results: HashMap<ServerName, Value>,
    client_name: String,
    client_version: String,
    protocol_version: String,
    protocol_version_check: ProtocolVersionCheck,
    protocol_version_mismatches: Vec<ProtocolVersionMismatch>,
    server_handler_timeout_counts: ServerHandlerTimeoutCounts,
    capabilities: Value,
    roots: Option<Arc<Vec<Root>>>,
    trust_mode: TrustMode,
    untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy,
    allow_stdout_log_outside_root: bool,
    request_timeout: Duration,
    server_handler_concurrency: usize,
    server_handler_timeout: Option<Duration>,
    server_request_handler: ServerRequestHandler,
    server_notification_handler: ServerNotificationHandler,
}

pub struct Connection {
    id: u64,
    child: Option<Child>,
    client: mcp_jsonrpc::Client,
    handler_tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub(crate) struct PreparedConnectedClient {
    pub server_name: String,
    pub connection_id: u64,
    pub timeout: Duration,
    pub client: mcp_jsonrpc::ClientHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedCallCleanup {
    None,
    Disconnect,
}

impl PreparedCallCleanup {
    pub(crate) const fn should_disconnect(self) -> bool {
        matches!(self, Self::Disconnect)
    }
}

pub(crate) struct PreparedCall<T> {
    result: anyhow::Result<T>,
    cleanup: PreparedCallCleanup,
}

impl<T> PreparedCall<T> {
    pub(crate) fn cleanup(&self) -> PreparedCallCleanup {
        self.cleanup
    }

    pub(crate) fn into_result(self) -> anyhow::Result<T> {
        self.result
    }
}

fn request_call_cleanup(result: &anyhow::Result<Value>) -> PreparedCallCleanup {
    result
        .as_ref()
        .err()
        .map_or(PreparedCallCleanup::None, |err| {
            if should_disconnect_after_jsonrpc_error(err) {
                PreparedCallCleanup::Disconnect
            } else {
                PreparedCallCleanup::None
            }
        })
}

fn notify_call_cleanup(result: &anyhow::Result<()>) -> PreparedCallCleanup {
    result
        .as_ref()
        .err()
        .map_or(PreparedCallCleanup::None, |err| {
            if should_disconnect_after_jsonrpc_error(err) || contains_wait_timeout(err) {
                PreparedCallCleanup::Disconnect
            } else {
                PreparedCallCleanup::None
            }
        })
}

impl PreparedConnectedClient {
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> anyhow::Result<Value> {
        Manager::request_raw_handle(
            self.timeout,
            &self.server_name,
            &self.client,
            method,
            params,
        )
        .await
    }

    pub(crate) async fn notify(&self, method: &str, params: Option<Value>) -> anyhow::Result<()> {
        Manager::notify_raw_handle(
            self.timeout,
            &self.server_name,
            &self.client,
            method,
            params,
        )
        .await
    }

    pub(crate) async fn request_call(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> PreparedCall<Value> {
        let result = self.request(method, params).await;
        PreparedCall {
            cleanup: request_call_cleanup(&result),
            result,
        }
    }

    pub(crate) async fn notify_call(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> PreparedCall<()> {
        let result = self.notify(method, params).await;
        PreparedCall {
            cleanup: notify_call_cleanup(&result),
            result,
        }
    }
}

pub(crate) struct PreparedTransportConnect {
    pub server_name: String,
    pub server_name_key: ServerName,
    pub server_cfg: ServerConfig,
    pub cwd: PathBuf,
    pub ctx: ConnectContext,
}

fn handler_task_join_error(err: tokio::task::JoinError) -> anyhow::Error {
    if err.is_panic() {
        anyhow::anyhow!("server handler task panicked")
    } else {
        anyhow::anyhow!("server handler task failed: {err}")
    }
}

#[cfg(test)]
mod prepared_call_tests {
    use super::{PreparedCallCleanup, notify_call_cleanup, request_call_cleanup};

    #[test]
    fn request_cleanup_ignores_wait_timeout_protocol_errors() {
        let result = Err(anyhow::Error::new(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::WaitTimeout,
            "timed out",
        )));

        assert_eq!(request_call_cleanup(&result), PreparedCallCleanup::None);
    }

    #[test]
    fn request_cleanup_disconnects_on_non_timeout_protocol_errors() {
        let result = Err(anyhow::Error::new(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::InvalidInput,
            "bad request",
        )));

        assert_eq!(
            request_call_cleanup(&result),
            PreparedCallCleanup::Disconnect
        );
    }

    #[test]
    fn notify_cleanup_disconnects_on_wait_timeout() {
        let result = Err(anyhow::Error::new(mcp_jsonrpc::Error::protocol(
            mcp_jsonrpc::ProtocolErrorKind::WaitTimeout,
            "timed out",
        )));

        assert_eq!(
            notify_call_cleanup(&result),
            PreparedCallCleanup::Disconnect
        );
    }
}

impl Connection {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub fn client(&self) -> &mcp_jsonrpc::Client {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut mcp_jsonrpc::Client {
        &mut self.client
    }

    pub fn child_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(tokio::process::Child::id)
    }

    pub fn take_child(&mut self) -> Option<Child> {
        self.child.take()
    }

    /// Closes the JSON-RPC client and (if present) waits for the underlying child process to exit.
    ///
    /// Note: this can hang indefinitely if the child process does not exit. Prefer
    /// `Connection::wait_with_timeout` if you need an upper bound.
    pub async fn wait(mut self) -> crate::Result<Option<std::process::ExitStatus>> {
        let status = self.client.wait().await.context("close jsonrpc client")?;
        let status = match status {
            Some(status) => Some(status),
            None => match &mut self.child {
                Some(child) => Some(child.wait().await?),
                None => None,
            },
        };

        let handler_tasks = std::mem::take(&mut self.handler_tasks);
        let mut first_handler_task_error: Option<anyhow::Error> = None;
        for task in handler_tasks {
            if first_handler_task_error.is_some() {
                task.abort();
                drop(task.await);
                continue;
            }

            if let Err(err) = task.await {
                first_handler_task_error = Some(handler_task_join_error(err));
            }
        }

        if let Some(err) = first_handler_task_error {
            return Err(err.into());
        }

        Ok(status)
    }

    /// Closes the JSON-RPC client and waits for the underlying child process to exit, up to
    /// `timeout`.
    ///
    /// This requires a Tokio runtime with the time driver enabled.
    pub async fn wait_with_timeout(
        mut self,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        ensure_tokio_time_driver("Connection::wait_with_timeout")?;
        let deadline = tokio::time::Instant::now() + timeout;
        let remaining_budget = || deadline.saturating_duration_since(tokio::time::Instant::now());
        let status = match self
            .client
            .wait_with_timeout(remaining_budget(), on_timeout)
            .await
        {
            Ok(status) => status,
            Err(err) => {
                let err = anyhow::Error::new(err).context("close jsonrpc client");
                if !contains_wait_timeout(&err) {
                    return Err(err.into());
                }

                match on_timeout {
                    mcp_jsonrpc::WaitOnTimeout::ReturnError => return Err(err.into()),
                    mcp_jsonrpc::WaitOnTimeout::Kill { kill_timeout } => {
                        let Some(child) = &mut self.child else {
                            return Err(err.into());
                        };
                        let child_id = child.id();
                        if let Err(kill_err) = child.start_kill() {
                            match child.try_wait() {
                                Ok(Some(status)) => Some(status),
                                Ok(None) => {
                                    timeout_bail!(
                                        "wait timed out after {timeout:?}; failed to kill detached child (id={child_id:?}): {kill_err}"
                                    )
                                }
                                Err(try_wait_err) => {
                                    timeout_bail!(
                                        "wait timed out after {timeout:?}; failed to kill detached child (id={child_id:?}): {kill_err}; try_wait failed: {try_wait_err}"
                                    )
                                }
                            }
                        } else {
                            match tokio::time::timeout(kill_timeout, child.wait()).await {
                                Ok(status) => Some(status?),
                                Err(_) => timeout_bail!(
                                    "wait timed out after {timeout:?}; killed detached child (id={child_id:?}) but it did not exit within {kill_timeout:?}"
                                ),
                            }
                        }
                    }
                }
            }
        };
        let status = match status {
            Some(status) => Some(status),
            None => match &mut self.child {
                Some(child) => match tokio::time::timeout_at(deadline, child.wait()).await {
                    Ok(status) => Some(status?),
                    Err(_) => match on_timeout {
                        mcp_jsonrpc::WaitOnTimeout::ReturnError => {
                            timeout_bail!("wait timed out after {timeout:?}")
                        }
                        mcp_jsonrpc::WaitOnTimeout::Kill { kill_timeout } => {
                            let child_id = child.id();
                            if let Err(err) = child.start_kill() {
                                match child.try_wait() {
                                    Ok(Some(status)) => Some(status),
                                    Ok(None) => {
                                        timeout_bail!(
                                            "wait timed out after {timeout:?}; failed to kill child (id={child_id:?}): {err}"
                                        )
                                    }
                                    Err(try_wait_err) => {
                                        timeout_bail!(
                                            "wait timed out after {timeout:?}; failed to kill child (id={child_id:?}): {err}; try_wait failed: {try_wait_err}"
                                        )
                                    }
                                }
                            } else {
                                match tokio::time::timeout(kill_timeout, child.wait()).await {
                                    Ok(status) => Some(status?),
                                    Err(_) => timeout_bail!(
                                        "wait timed out after {timeout:?}; killed child (id={child_id:?}) but it did not exit within {kill_timeout:?}"
                                    ),
                                }
                            }
                        }
                    },
                },
                None => None,
            },
        };

        let handler_tasks = std::mem::take(&mut self.handler_tasks);
        let mut first_handler_task_error: Option<anyhow::Error> = None;
        for mut task in handler_tasks {
            if first_handler_task_error.is_some() {
                task.abort();
                let cleanup_timeout = match on_timeout {
                    mcp_jsonrpc::WaitOnTimeout::ReturnError => remaining_budget(),
                    mcp_jsonrpc::WaitOnTimeout::Kill { kill_timeout } => kill_timeout,
                };
                if !cleanup_timeout.is_zero() {
                    let _ = tokio::time::timeout(cleanup_timeout, task).await; // pre-commit: allow-let-underscore
                }
                continue;
            }

            match tokio::time::timeout_at(deadline, &mut task).await {
                Ok(join_result) => {
                    if let Err(err) = join_result {
                        first_handler_task_error = Some(handler_task_join_error(err));
                    }
                }
                Err(_) => match on_timeout {
                    mcp_jsonrpc::WaitOnTimeout::ReturnError => {
                        first_handler_task_error = Some(tagged_message(
                            ErrorKind::Timeout,
                            format!(
                                "wait timed out after {timeout:?} while waiting for server handler task"
                            ),
                        ));
                        task.abort();
                        let cleanup_timeout = remaining_budget();
                        if !cleanup_timeout.is_zero() {
                            let _ = tokio::time::timeout(cleanup_timeout, task).await; // pre-commit: allow-let-underscore
                        }
                    }
                    mcp_jsonrpc::WaitOnTimeout::Kill { kill_timeout } => {
                        task.abort();
                        match tokio::time::timeout(kill_timeout, task).await {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) if err.is_cancelled() => {}
                            Ok(Err(err)) => {
                                first_handler_task_error = Some(handler_task_join_error(err));
                            }
                            Err(_) => {
                                first_handler_task_error = Some(tagged_message(
                                    ErrorKind::Timeout,
                                    format!(
                                        "wait timed out after {timeout:?}; aborted server handler task but it did not stop within {kill_timeout:?}"
                                    ),
                                ));
                            }
                        }
                    }
                },
            }
        }

        if let Some(err) = first_handler_task_error {
            return Err(err.into());
        }

        Ok(status)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in self.handler_tasks.drain(..) {
            task.abort();
        }
        Manager::reap_connection_child_best_effort(self);
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new(
            "mcp-kit",
            env!("CARGO_PKG_VERSION"),
            Duration::from_secs(30),
        )
    }
}

impl Manager {
    #[cfg(test)]
    pub(crate) fn active_handler_scopes(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.active_handler_scopes)
    }

    pub fn try_from_config(
        config: &Config,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
        timeout: Duration,
    ) -> crate::Result<Self> {
        config.validate()?;
        Ok(Self::from_config(
            config,
            client_name,
            client_version,
            timeout,
        ))
    }

    /// Build a `Manager` using client defaults from `config`.
    ///
    /// Note: this constructor still fail-fast validates the full config and will panic if
    /// `config` is invalid. Use `Manager::try_from_config` if you want a typed validation error.
    pub fn from_config(
        config: &Config,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        if let Err(err) = config.validate() {
            panic!(
                "Manager::from_config requires a validated Config (use try_from_config): {err:#}"
            );
        }
        let mut manager = Self::new(client_name, client_version, timeout);
        if let Some(protocol_version) = config.client().protocol_version.clone() {
            manager = manager
                .with_protocol_version(protocol_version)
                .expect("validated Config should always carry a non-empty protocol version");
        }
        if let Some(capabilities) = config.client().capabilities.clone() {
            manager = manager
                .with_capabilities(capabilities)
                .expect("validated Config should always carry object-shaped client capabilities");
        }
        if let Some(roots) = config.client().roots.clone() {
            manager = manager.with_roots(roots);
        }
        manager
    }

    pub fn new(
        client_name: impl Into<String>,
        client_version: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        let server_request_handler: ServerRequestHandler =
            Arc::new(|_| Box::pin(async { ServerRequestOutcome::MethodNotFound }));
        let server_notification_handler: ServerNotificationHandler =
            Arc::new(|_| Box::pin(async {}));

        Self {
            instance_id: NEXT_MANAGER_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
            active_handler_scopes: Arc::new(AtomicU64::new(0)),
            conns: HashMap::new(),
            connection_cwds: HashMap::new(),
            connection_server_configs: HashMap::new(),
            init_results: HashMap::new(),
            client_name: client_name.into(),
            client_version: client_version.into(),
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            protocol_version_check: ProtocolVersionCheck::Strict,
            protocol_version_mismatches: Vec::new(),
            server_handler_timeout_counts: ServerHandlerTimeoutCounts::default(),
            capabilities: Value::Object(serde_json::Map::new()),
            roots: None,
            trust_mode: TrustMode::Untrusted,
            untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy::default(),
            allow_stdout_log_outside_root: false,
            request_timeout: timeout,
            server_handler_concurrency: 1,
            server_handler_timeout: None,
            server_request_handler,
            server_notification_handler,
        }
    }

    pub fn with_trust_mode(mut self, trust_mode: TrustMode) -> Self {
        self.trust_mode = trust_mode;
        self
    }

    pub fn with_untrusted_streamable_http_policy(
        mut self,
        policy: UntrustedStreamableHttpPolicy,
    ) -> Self {
        self.untrusted_streamable_http_policy = policy;
        self
    }

    pub fn with_allow_stdout_log_outside_root(mut self, allow: bool) -> Self {
        self.allow_stdout_log_outside_root = allow;
        self
    }

    pub fn trust_mode(&self) -> TrustMode {
        self.trust_mode
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Override the protocol version advertised during `initialize`.
    ///
    /// Returns a config error when `protocol_version` is blank after trimming.
    pub fn with_protocol_version(
        mut self,
        protocol_version: impl Into<String>,
    ) -> crate::Result<Self> {
        self.protocol_version = validate_protocol_version(protocol_version)?;
        Ok(self)
    }

    pub fn with_protocol_version_check(mut self, check: ProtocolVersionCheck) -> Self {
        self.protocol_version_check = check;
        self
    }

    pub(crate) fn instance_id(&self) -> u64 {
        self.instance_id
    }

    pub fn protocol_version_mismatches(&self) -> &[ProtocolVersionMismatch] {
        &self.protocol_version_mismatches
    }

    pub fn take_protocol_version_mismatches(&mut self) -> Vec<ProtocolVersionMismatch> {
        std::mem::take(&mut self.protocol_version_mismatches)
    }

    fn remove_protocol_version_mismatch(&mut self, server_name: &str) {
        let server_name = normalize_server_name_lookup(server_name);
        self.protocol_version_mismatches
            .retain(|mismatch| mismatch.server_name.as_str() != server_name);
    }

    /// Returns the number of server→client handler timeouts observed for `server_name`.
    ///
    /// This increments when a server→client request/notification handler exceeds
    /// `Manager::with_server_handler_timeout(...)`.
    pub fn server_handler_timeout_count(&self, server_name: &str) -> u64 {
        self.server_handler_timeout_counts
            .count(normalize_server_name_lookup(server_name))
    }

    /// Returns a snapshot of timeout counts for all servers.
    pub fn server_handler_timeout_counts(&self) -> HashMap<ServerName, u64> {
        self.server_handler_timeout_counts.snapshot()
    }

    /// Takes a snapshot of timeout counts and resets all counters to zero.
    ///
    /// Stale zeroed entries are compacted after reset. Active handler tasks can still keep
    /// shared counters alive, so `server_handler_timeout_counts()` may include servers with a
    /// count of 0.
    pub fn take_server_handler_timeout_counts(&mut self) -> HashMap<ServerName, u64> {
        self.server_handler_timeout_counts.take_and_reset()
    }

    /// Override the client capabilities advertised during `initialize`.
    ///
    /// Returns a config error when `capabilities` is not a JSON object.
    pub fn with_capabilities(mut self, capabilities: Value) -> crate::Result<Self> {
        self.capabilities = validate_capabilities(capabilities)?;
        if self.roots.is_some() {
            ensure_roots_capability(&mut self.capabilities);
        }
        Ok(self)
    }

    pub fn with_roots(mut self, roots: Vec<Root>) -> Self {
        self.roots = Some(Arc::new(roots));
        ensure_roots_capability(&mut self.capabilities);
        self
    }

    pub fn with_server_request_handler(mut self, handler: ServerRequestHandler) -> Self {
        self.server_request_handler = handler;
        self
    }

    pub fn with_server_notification_handler(mut self, handler: ServerNotificationHandler) -> Self {
        self.server_notification_handler = handler;
        self
    }

    /// Set the maximum number of in-flight server→client handler calls per connection.
    ///
    /// Default: 1 (sequential handling).
    pub fn with_server_handler_concurrency(mut self, concurrency: usize) -> Self {
        self.server_handler_concurrency = concurrency.max(1);
        self
    }

    /// Set a per-message timeout for server→client request/notification handlers.
    ///
    /// Default: no timeout.
    pub fn with_server_handler_timeout(mut self, timeout: Duration) -> Self {
        self.server_handler_timeout = Some(timeout);
        self
    }

    pub fn without_server_handler_timeout(mut self) -> Self {
        self.server_handler_timeout = None;
        self
    }

    pub fn is_connected(&mut self, server_name: &str) -> bool {
        let server_name = normalize_server_name_lookup(server_name);
        let connected = self.is_connected_and_alive(server_name);
        if !connected {
            self.clear_connection_cwd(server_name);
        }
        connected
    }

    pub fn is_connected_named(&mut self, server_name: &ServerName) -> bool {
        self.is_connected(server_name.as_str())
    }

    pub fn connected_server_names(&mut self) -> Vec<ServerName> {
        let mut names = self.conns.keys().cloned().collect::<Vec<_>>();
        names.retain(|name| {
            let connected = self.is_connected_and_alive(name.as_str());
            if !connected {
                self.clear_connection_cwd(name.as_str());
            }
            connected
        });
        names
    }

    pub fn initialize_result(&self, server_name: &str) -> Option<&Value> {
        self.init_results
            .get(normalize_server_name_lookup(server_name))
    }

    pub fn initialize_result_named(&self, server_name: &ServerName) -> Option<&Value> {
        self.initialize_result(server_name.as_str())
    }

    pub(crate) fn record_connection_cwd(
        &mut self,
        server_name: &str,
        cwd: &Path,
    ) -> anyhow::Result<()> {
        self.record_connection_cwd_with_base(server_name, cwd, None)
    }

    pub(crate) fn record_connection_cwd_with_base(
        &mut self,
        server_name: &str,
        cwd: &Path,
        base: Option<&Path>,
    ) -> anyhow::Result<()> {
        let server_name = parse_server_name_anyhow(server_name)?;
        self.connection_cwds
            .insert(server_name, resolve_connection_cwd_with_base(base, cwd)?);
        Ok(())
    }

    pub(crate) fn clear_connection_cwd(&mut self, server_name: &str) {
        self.connection_cwds.remove(server_name);
        self.clear_connection_server_config(server_name);
    }

    #[cfg(test)]
    pub(crate) fn record_connection_server_config(
        &mut self,
        server_name: &str,
        server_config: &ServerConfig,
    ) -> anyhow::Result<()> {
        let server_name = parse_server_name_anyhow(server_name)?;
        self.connection_server_configs
            .insert(server_name, raw_server_config_identity(server_config));
        Ok(())
    }

    pub(crate) fn clear_connection_server_config(&mut self, server_name: &str) {
        self.connection_server_configs.remove(server_name);
    }

    fn ensure_connection_server_config_matches(
        &self,
        server_name: &str,
        requested: &ConnectionServerConfigIdentity,
    ) -> anyhow::Result<()> {
        let Some(connected) = self.connection_server_configs.get(server_name) else {
            return Err(tagged_message(
                ErrorKind::ManagerState,
                format!(
                    "mcp server {server_name} is already connected without reusable config metadata and cannot be reused from config (disconnect first)"
                ),
            ));
        };
        if connected == requested {
            return Ok(());
        }

        Err(tagged_message(
            ErrorKind::ManagerState,
            "mcp server {server_name} is already connected with a different effective config and cannot be reused (disconnect first)",
        ))
    }

    fn ensure_connection_cwd_matches(
        &self,
        server_name: &str,
        cwd: &Path,
        base: Option<&Path>,
    ) -> anyhow::Result<()> {
        let Some(connected_cwd) = self.connection_cwds.get(server_name) else {
            return Ok(());
        };
        let requested_cwd = resolve_connection_cwd_with_base(base, cwd)?;
        if *connected_cwd == requested_cwd {
            return Ok(());
        }

        Err(tagged_message(
            ErrorKind::ManagerState,
            format!(
                "mcp server {server_name} is already connected under cwd={} and cannot be reused for cwd={} (disconnect first)",
                connected_cwd.display(),
                requested_cwd.display()
            ),
        ))
    }

    pub(crate) fn try_prepare_connected_client(
        &mut self,
        server_name: &str,
        cwd: Option<&Path>,
    ) -> anyhow::Result<Option<PreparedConnectedClient>> {
        let server_name = normalize_server_name_lookup(server_name);
        if !self.is_connected_and_alive(server_name) {
            self.clear_connection_cwd(server_name);
            return Ok(None);
        }
        if let Some(cwd) = cwd {
            self.ensure_connection_cwd_matches(server_name, cwd, None)?;
        }

        let conn = self.conns.get(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?;
        Ok(Some(PreparedConnectedClient {
            server_name: server_name.to_string(),
            connection_id: conn.id(),
            timeout: self.request_timeout,
            client: conn.client.handle(),
        }))
    }

    pub(crate) fn try_prepare_reusable_connected_client(
        &mut self,
        server_name: &str,
        server_cfg: &ServerConfig,
        cwd: Option<&Path>,
    ) -> anyhow::Result<Option<PreparedConnectedClient>> {
        let prepared = self.try_prepare_connected_client(server_name, cwd)?;
        let Some(prepared) = prepared else {
            return Ok(None);
        };
        let requested = if let Some(cwd) = cwd {
            effective_server_config_identity(
                &self.connect_context_for_identity(),
                server_name,
                server_cfg,
                cwd,
            )?
        } else {
            raw_server_config_identity(server_cfg)
        };
        self.ensure_connection_server_config_matches(prepared.server_name.as_str(), &requested)?;
        Ok(Some(prepared))
    }

    fn connect_context_for_identity(&self) -> ConnectContext {
        ConnectContext {
            trust_mode: self.trust_mode,
            untrusted_streamable_http_policy: self.untrusted_streamable_http_policy.clone(),
            allow_stdout_log_outside_root: self.allow_stdout_log_outside_root,
            stdout_log_root: None,
            protocol_version: self.protocol_version.clone(),
            request_timeout: self.request_timeout,
        }
    }

    pub(crate) fn prepare_transport_connect(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> anyhow::Result<Option<PreparedTransportConnect>> {
        let server_cfg = config.server(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::Config,
                format!("unknown mcp server: {server_name}"),
            )
        })?;
        let server_name_key = parse_server_name_anyhow(server_name)?;
        let config_root = config.thread_root();
        let cwd = resolve_config_connection_cwd(config_root, cwd)?;
        let ctx = ConnectContext {
            trust_mode: self.trust_mode,
            untrusted_streamable_http_policy: self.untrusted_streamable_http_policy.clone(),
            allow_stdout_log_outside_root: self.allow_stdout_log_outside_root,
            stdout_log_root: config_root.map(Path::to_path_buf),
            protocol_version: self.protocol_version.clone(),
            request_timeout: self.request_timeout,
        };
        if self.is_connected_and_alive(server_name_key.as_str()) {
            let server_cfg_identity =
                effective_server_config_identity(&ctx, server_name, server_cfg, &cwd)?;
            self.ensure_connection_cwd_matches(server_name_key.as_str(), &cwd, None)?;
            self.ensure_connection_server_config_matches(
                server_name_key.as_str(),
                &server_cfg_identity,
            )?;
            return Ok(None);
        }
        self.clear_connection_cwd(server_name_key.as_str());

        server_cfg
            .validate()
            .with_context(|| format!("invalid mcp server config (server={server_name_key})"))
            .map_err(|err| wrap_kind(ErrorKind::Config, err))?;

        Ok(Some(PreparedTransportConnect {
            server_name: server_name.to_string(),
            server_name_key,
            server_cfg: server_cfg.clone(),
            cwd,
            ctx,
        }))
    }

    pub(crate) fn prepare_disconnect_for_wait_with_cwd_cleanup(
        &mut self,
        server_name: &str,
    ) -> crate::manager::lifecycle::PreparedDisconnect {
        let normalized = normalize_server_name_lookup(server_name).to_string();
        let should_clear = self.conns.contains_key(normalized.as_str());
        let disconnect = self.prepare_disconnect_for_wait(&normalized);
        if should_clear {
            self.clear_connection_cwd(&normalized);
        }
        disconnect
    }

    pub(crate) fn prepare_disconnect_for_wait_if_connection_with_cwd_cleanup(
        &mut self,
        server_name: &str,
        connection_id: u64,
    ) -> crate::manager::lifecycle::PreparedDisconnect {
        let normalized = normalize_server_name_lookup(server_name).to_string();
        let should_clear = self
            .conns
            .get(normalized.as_str())
            .is_some_and(|conn| conn.id() == connection_id);
        let disconnect = self.prepare_disconnect_for_wait_if_connection(&normalized, connection_id);
        if should_clear {
            self.clear_connection_cwd(&normalized);
        }
        disconnect
    }

    pub async fn connect(
        &mut self,
        server_name: &str,
        server_cfg: &ServerConfig,
        cwd: &Path,
    ) -> crate::Result<()> {
        Ok(self
            .connect_with_builder(server_name, server_cfg, cwd, None, || {
                parse_server_name_anyhow(server_name)
            })
            .await?)
    }

    async fn connect_with_builder<F>(
        &mut self,
        server_name: &str,
        server_cfg: &ServerConfig,
        cwd: &Path,
        cwd_base: Option<&Path>,
        build_server_name: F,
    ) -> anyhow::Result<()>
    where
        F: FnOnce() -> anyhow::Result<ServerName>,
    {
        let cwd = resolve_connection_cwd_with_base(cwd_base, cwd)?;
        let server_name_key = build_server_name()?;
        let ctx = ConnectContext {
            trust_mode: self.trust_mode,
            untrusted_streamable_http_policy: self.untrusted_streamable_http_policy.clone(),
            allow_stdout_log_outside_root: self.allow_stdout_log_outside_root,
            stdout_log_root: Some(cwd.clone()),
            protocol_version: self.protocol_version.clone(),
            request_timeout: self.request_timeout,
        };
        if self.is_connected_and_alive(server_name_key.as_str()) {
            let server_cfg_identity =
                effective_server_config_identity(&ctx, server_name, server_cfg, &cwd)?;
            self.ensure_connection_cwd_matches(server_name_key.as_str(), &cwd, None)?;
            self.ensure_connection_server_config_matches(
                server_name_key.as_str(),
                &server_cfg_identity,
            )?;
            return Ok(());
        }
        self.clear_connection_cwd(server_name_key.as_str());

        server_cfg
            .validate()
            .with_context(|| format!("invalid mcp server config (server={server_name_key})"))?;

        let lifecycle = self.prepare_transport_lifecycle(PreparedTransportConnect {
            server_name: server_name.to_string(),
            server_name_key,
            server_cfg: server_cfg.clone(),
            cwd: cwd.clone(),
            ctx,
        });
        self.finish_transport_lifecycle(lifecycle.run().await)
    }

    /// Attach an already-connected `mcp_jsonrpc::Client` and perform MCP initialize.
    ///
    /// This requires `TrustMode::Trusted` because attaching a custom client can bypass
    /// `Untrusted`-mode safety checks (for example, by constructing a custom streamable_http
    /// client with different redirect/proxy/header behavior).
    pub async fn connect_jsonrpc(
        &mut self,
        server_name: &str,
        client: mcp_jsonrpc::Client,
    ) -> crate::Result<()> {
        if self.trust_mode == TrustMode::Untrusted {
            config_bail!(
                "refusing to attach custom JSON-RPC client in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) or use Manager::connect_jsonrpc_unchecked)"
            );
        }

        self.connect_jsonrpc_unchecked(server_name, client).await
    }

    /// Like `Manager::connect_jsonrpc`, but does not enforce `TrustMode`.
    ///
    /// This is intended for controlled environments (e.g. tests) where you explicitly accept the
    /// risk of bypassing `Untrusted`-mode safety checks.
    pub async fn connect_jsonrpc_unchecked(
        &mut self,
        server_name: &str,
        client: mcp_jsonrpc::Client,
    ) -> crate::Result<()> {
        Ok(self
            .connect_jsonrpc_with_builder(
                server_name,
                || parse_server_name_anyhow(server_name),
                client,
            )
            .await?)
    }

    async fn connect_jsonrpc_with_builder<F>(
        &mut self,
        _server_name: &str,
        build_server_name: F,
        mut client: mcp_jsonrpc::Client,
    ) -> anyhow::Result<()>
    where
        F: FnOnce() -> anyhow::Result<ServerName>,
    {
        let server_name_key = build_server_name()?;
        if self.is_connected_and_alive(server_name_key.as_str()) {
            return Err(reject_duplicate_custom_jsonrpc_client(
                &server_name_key,
                &mut client,
            ));
        }
        self.clear_connection_cwd(server_name_key.as_str());

        let child = client.take_child();
        self.install_connection_parsed(server_name_key, client, child)
            .await?;
        Ok(())
    }

    /// Attach a custom `AsyncRead + AsyncWrite` transport as a JSON-RPC connection and perform
    /// MCP initialize.
    ///
    /// This requires `TrustMode::Trusted` because attaching a custom transport can bypass
    /// `Untrusted`-mode safety checks.
    pub async fn connect_io<R, W>(
        &mut self,
        server_name: &str,
        read: R,
        write: W,
    ) -> crate::Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        if self.trust_mode == TrustMode::Untrusted {
            config_bail!(
                "refusing to attach custom JSON-RPC IO in untrusted mode: {server_name} (set Manager::with_trust_mode(TrustMode::Trusted) or use Manager::connect_io_unchecked)"
            );
        }

        self.connect_io_unchecked(server_name, read, write).await
    }

    /// Like `Manager::connect_io`, but does not enforce `TrustMode`.
    ///
    /// This is intended for controlled environments (e.g. tests) where you explicitly accept the
    /// risk of bypassing `Untrusted`-mode safety checks.
    pub async fn connect_io_unchecked<R, W>(
        &mut self,
        server_name: &str,
        read: R,
        write: W,
    ) -> crate::Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let server_name_key = parse_server_name_anyhow(server_name)?;
        if self.is_connected_and_alive(server_name_key.as_str()) {
            let _ = read;
            let _ = write;
            return Err(duplicate_live_connection_error(
                &server_name_key,
                "custom JSON-RPC IO transport",
            )
            .into());
        }
        self.clear_connection_cwd(server_name_key.as_str());

        let client = mcp_jsonrpc::Client::connect_io(read, write)
            .await
            .context("connect jsonrpc io")?;
        Ok(self
            .install_connection_parsed(server_name_key, client, None)
            .await?)
    }

    pub async fn get_or_connect(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<()> {
        let server_cfg = config.server(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::Config,
                format!("unknown mcp server: {server_name}"),
            )
        })?;
        Ok(self
            .connect_with_builder(server_name, server_cfg, cwd, config.thread_root(), || {
                parse_server_name_anyhow(server_name)
            })
            .await?)
    }

    pub async fn get_or_connect_named(
        &mut self,
        config: &Config,
        server_name: &ServerName,
        cwd: &Path,
    ) -> crate::Result<()> {
        let server_cfg = config.server_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::Config,
                format!("unknown mcp server: {server_name}"),
            )
        })?;
        let server_name_key = server_name.clone();
        Ok(self
            .connect_with_builder(
                server_name.as_str(),
                server_cfg,
                cwd,
                config.thread_root(),
                || Ok(server_name_key),
            )
            .await?)
    }

    pub async fn get_or_connect_session(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<Session> {
        self.get_or_connect(config, server_name, cwd).await?;
        Ok(self.take_session(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn get_or_connect_session_named(
        &mut self,
        config: &Config,
        server_name: &ServerName,
        cwd: &Path,
    ) -> crate::Result<Session> {
        self.get_or_connect_named(config, server_name, cwd).await?;
        Ok(self.take_session_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_named(
        &mut self,
        server_name: &ServerName,
        server_cfg: &ServerConfig,
        cwd: &Path,
    ) -> crate::Result<()> {
        let server_name_key = server_name.clone();
        Ok(self
            .connect_with_builder(server_name.as_str(), server_cfg, cwd, None, || {
                Ok(server_name_key)
            })
            .await?)
    }

    /// Remove a cached connection (if any) without waiting for shutdown.
    ///
    /// This performs best-effort child-process reaping, but still does not guarantee clean
    /// shutdown ordering. Prefer `Manager::disconnect_and_wait` (or `take_connection` +
    /// `Connection::wait_with_timeout`) when you own the lifecycle.
    pub fn disconnect(&mut self, server_name: &str) -> bool {
        let server_name = normalize_server_name_lookup(server_name);
        let removed = self.remove_cached_connection(server_name);
        self.clear_connection_cwd(server_name);
        self.clear_connection_side_state(server_name, true);
        if let Some(mut conn) = removed {
            Self::reap_connection_child_best_effort(&mut conn);
            true
        } else {
            false
        }
    }

    pub fn disconnect_named(&mut self, server_name: &ServerName) -> bool {
        self.disconnect(server_name.as_str())
    }

    pub async fn disconnect_and_wait(
        &mut self,
        server_name: &str,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        Ok(self
            .prepare_disconnect_for_wait_with_cwd_cleanup(server_name)
            .wait_with_timeout(timeout, on_timeout)
            .await?)
    }

    pub async fn disconnect_and_wait_named(
        &mut self,
        server_name: &ServerName,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        self.disconnect_and_wait(server_name.as_str(), timeout, on_timeout)
            .await
    }

    /// Take ownership of a cached connection (if any).
    ///
    /// After calling this, the caller owns the connection lifecycle. In particular, if the
    /// connection was created via `transport=stdio`, prefer an explicit `Connection::wait*` call
    /// to avoid leaving a child process running/zombied.
    pub fn take_connection(&mut self, server_name: &str) -> Option<Connection> {
        let server_name = normalize_server_name_lookup(server_name);
        self.init_results.remove(server_name);
        let conn = self.remove_cached_connection(server_name);
        self.clear_connection_cwd(server_name);
        if conn.is_some() {
            self.clear_connection_side_state(server_name, false);
        }
        conn
    }

    pub fn take_connection_named(&mut self, server_name: &ServerName) -> Option<Connection> {
        self.take_connection(server_name.as_str())
    }

    /// Take ownership of a cached session (if any).
    ///
    /// After calling this, the caller owns the session lifecycle. Prefer calling
    /// `Session::wait_with_timeout` (or converting into a `Connection` and calling `wait*`) to
    /// ensure any associated stdio child process is reaped.
    ///
    /// Note: this does not clear manager-local telemetry/state (e.g. protocol-version mismatch
    /// records or timeout counters). If you want to clear retained state after handoff, call
    /// `Manager::disconnect` / `Manager::disconnect_and_wait` for the same server name.
    pub fn take_session(&mut self, server_name: &str) -> Option<Session> {
        let server_name = normalize_server_name_lookup(server_name);
        self.clear_connection_cwd(server_name);
        let Some((server_name, connection)) = self.conns.remove_entry(server_name) else {
            self.init_results.remove(server_name);
            return None;
        };
        let Some(initialize_result) = self.init_results.remove(&server_name) else {
            self.conns.insert(server_name, connection);
            return None;
        };
        Some(Session::new(
            server_name,
            connection,
            initialize_result,
            self.request_timeout,
        ))
    }

    pub fn take_session_named(&mut self, server_name: &ServerName) -> Option<Session> {
        self.take_session(server_name.as_str())
    }

    /// Take ownership of a cached session (if any) and clear manager-local telemetry/state.
    ///
    /// Unlike `Manager::take_session`, this also clears timeout counters and protocol-version
    /// mismatch records for the target server name.
    pub fn take_session_and_clear_state(&mut self, server_name: &str) -> Option<Session> {
        let session = self.take_session(server_name);
        let server_name = normalize_server_name_lookup(server_name);
        self.clear_connection_side_state(server_name, false);
        session
    }

    pub fn take_session_and_clear_state_named(
        &mut self,
        server_name: &ServerName,
    ) -> Option<Session> {
        self.take_session_and_clear_state(server_name.as_str())
    }

    pub async fn request_connected(
        &mut self,
        server_name: &str,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        let Some(prepared) = self.try_prepare_connected_client(server_name, None)? else {
            let server_name = normalize_server_name_lookup(server_name);
            manager_state_bail!("mcp server not connected: {server_name}");
        };
        let server_name = prepared.server_name.clone();
        let call = prepared.request_call(method, params).await;

        if call.cleanup().should_disconnect() {
            self.disconnect_after_jsonrpc_error(&server_name).await;
            self.clear_connection_cwd(&server_name);
        }

        Ok(call.into_result()?)
    }

    pub async fn request_connected_named(
        &mut self,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        self.request_connected(server_name.as_str(), method, params)
            .await
    }

    pub async fn notify_connected(
        &mut self,
        server_name: &str,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        let Some(prepared) = self.try_prepare_connected_client(server_name, None)? else {
            let server_name = normalize_server_name_lookup(server_name);
            manager_state_bail!("mcp server not connected: {server_name}");
        };
        let server_name = prepared.server_name.clone();
        let call = prepared.notify_call(method, params).await;

        if call.cleanup().should_disconnect() {
            self.disconnect_after_jsonrpc_error(&server_name).await;
            self.clear_connection_cwd(&server_name);
        }

        Ok(call.into_result()?)
    }

    pub async fn notify_connected_named(
        &mut self,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        self.notify_connected(server_name.as_str(), method, params)
            .await
    }

    pub(crate) async fn request_raw_handle(
        timeout: Duration,
        server_name: &str,
        client: &mcp_jsonrpc::ClientHandle,
        method: &str,
        params: Option<Value>,
    ) -> anyhow::Result<Value> {
        let result = client
            .request_optional_with_timeout(method, params, timeout)
            .await;
        match result {
            Ok(value) => Ok(value),
            Err(err) if err.is_wait_timeout() => Err(anyhow::Error::new(err).context(format!(
                "mcp request timed out after {timeout:?}: {method} (server={server_name})"
            ))),
            Err(err) => Err(anyhow::Error::new(err).context(format!(
                "mcp request failed: {method} (server={server_name})"
            ))),
        }
    }

    async fn notify_raw(
        timeout: Duration,
        server_name: &str,
        client: &mcp_jsonrpc::Client,
        method: &str,
        params: Option<Value>,
    ) -> anyhow::Result<()> {
        Self::notify_raw_handle(timeout, server_name, &client.handle(), method, params).await
    }

    pub(crate) async fn notify_raw_handle(
        timeout: Duration,
        server_name: &str,
        client: &mcp_jsonrpc::ClientHandle,
        method: &str,
        params: Option<Value>,
    ) -> anyhow::Result<()> {
        ensure_tokio_time_driver("Manager::notify_raw_handle")?;
        let outcome = tokio::time::timeout(timeout, client.notify(method, params)).await;
        match outcome {
            Ok(result) => result.with_context(|| {
                format!("mcp notification failed: {method} (server={server_name})")
            }),
            Err(_) => Err(anyhow::Error::new(mcp_jsonrpc::Error::protocol(
                mcp_jsonrpc::ProtocolErrorKind::WaitTimeout,
                format!(
                    "mcp notification timed out after {timeout:?}: {method} (server={server_name})"
                ),
            ))),
        }
    }
}

fn ensure_roots_capability(capabilities: &mut Value) {
    if !capabilities.is_object() {
        *capabilities = Value::Object(serde_json::Map::new());
    }
    let Value::Object(map) = capabilities else {
        unreachable!("non-object capabilities should be normalized before root injection");
    };
    match map.get_mut("roots") {
        Some(Value::Object(_)) => {}
        _ => {
            map.insert("roots".to_string(), Value::Object(serde_json::Map::new()));
        }
    }
}

#[cfg(test)]
mod tests;
