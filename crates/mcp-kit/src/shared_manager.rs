use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{Mutex, MutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::error::{ErrorKind, tagged_message};
use crate::{Config, Manager, McpNotification, McpRequest, ProtocolVersionMismatch, ServerName};

const REENTRANT_HANDLER_ERROR: &str = "SharedManager async operations cannot be called reentrantly from the wrapped manager's server handlers while another operation is in flight";

/// Cloneable wrapper around `Manager` for serialized shared async use.
///
/// This wrapper is intentionally a single-flight lifecycle gate, not an actor. It serializes
/// manager state access through a single async mutex, while same-server connect/disconnect paths
/// also share a per-server gate so cold starts and teardown cannot overlap. Connected
/// request/notify operations only hold that same-server gate long enough to borrow the current
/// connection and register an in-flight I/O guard, so sibling request/notify calls can overlap
/// while same-server disconnect still waits for the borrowed I/O to drain. Lifecycle and
/// inspection operations still execute under the shared lock.
///
/// Reentrant fail-fast is scoped to the current manager handler task, not to global handler
/// activity. If some other handler for the same `Manager` is active elsewhere, external callers
/// still wait normally for the shared lock or connect gate.
///
/// This is not an actor or fully concurrent manager:
/// - lifecycle-changing operations still serialize on the shared manager lock
/// - connected request/notify operations can overlap with each other while still blocking
///   same-server disconnect until their I/O finishes
/// - operations that still need the shared lock return an error on reentrant calls from
///   `Manager` server request/notification handlers instead of waiting forever
/// - connect/disconnect lifecycle changes for the same server share a single gate, and
///   `disconnect_and_wait` keeps that gate until its wait finishes so a slow teardown cannot race
///   with a replacement cold start
pub struct SharedManager {
    inner: Arc<Mutex<Manager>>,
    server_states: Arc<StdMutex<HashMap<ServerName, Weak<ServerState>>>>,
    manager_id: u64,
    captured_handler_scope: Option<Weak<()>>,
}

impl Manager {
    /// Converts this manager into a cloneable single-flight wrapper.
    pub fn into_shared(self) -> SharedManager {
        SharedManager::new(self)
    }
}

impl SharedManager {
    fn parse_server_name(server_name: &str) -> anyhow::Result<ServerName> {
        Ok(ServerName::parse(server_name)?)
    }

    pub fn new(manager: Manager) -> Self {
        let manager_id = manager.instance_id();
        Self {
            inner: Arc::new(Mutex::new(manager)),
            server_states: Arc::new(StdMutex::new(HashMap::new())),
            manager_id,
            captured_handler_scope: None,
        }
    }

    pub fn try_unwrap(self) -> Result<Manager, Self> {
        match Arc::try_unwrap(self.inner) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(inner) => Err(Self {
                inner,
                server_states: self.server_states,
                manager_id: self.manager_id,
                captured_handler_scope: self.captured_handler_scope,
            }),
        }
    }

    fn is_reentrant_handler_call(&self) -> bool {
        crate::manager::is_in_manager_handler_scope(self.manager_id)
            || self
                .captured_handler_scope
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some()
    }

    fn fail_fast_if_reentrant<T>(
        &self,
        operation: &'static str,
        try_acquire: impl FnOnce() -> Result<T, tokio::sync::TryLockError>,
    ) -> anyhow::Result<Option<T>> {
        // Only the current task-local manager handler scope gets fail-fast behavior. Other
        // unrelated handler activity must not cause external callers to spuriously error.
        if !self.is_reentrant_handler_call() {
            return Ok(None);
        }

        try_acquire().map(Some).map_err(|_| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("{REENTRANT_HANDLER_ERROR}: {operation}"),
            )
        })
    }

    fn server_state_for(&self, server_name: &ServerName) -> Arc<ServerState> {
        let mut states = self
            .server_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        states.retain(|_, state| state.strong_count() > 0);
        if let Some(existing) = states.get(server_name).and_then(Weak::upgrade) {
            return existing;
        }

        let state = Arc::new(ServerState::default());
        states.insert(server_name.clone(), Arc::downgrade(&state));
        state
    }

    async fn lock_for_async_op(
        &self,
        operation: &'static str,
    ) -> anyhow::Result<MutexGuard<'_, Manager>> {
        if let Some(manager) = self.fail_fast_if_reentrant(operation, || self.inner.try_lock())? {
            return Ok(manager);
        }
        Ok(self.inner.lock().await)
    }

    async fn with_manager_lock<R>(
        &self,
        operation: &'static str,
        f: impl FnOnce(&mut Manager) -> R,
    ) -> anyhow::Result<R> {
        let mut manager = self.lock_for_async_op(operation).await?;
        Ok(f(&mut manager))
    }

    async fn lock_connect_gate_write(
        &self,
        operation: &'static str,
        server_name: &ServerName,
    ) -> anyhow::Result<ServerLifecycleWriteGuard> {
        let state = self.server_state_for(server_name);
        let gate = Arc::clone(&state.lifecycle_gate);
        if let Some(guard) =
            self.fail_fast_if_reentrant(operation, || gate.clone().try_write_owned())?
        {
            return Ok(ServerLifecycleWriteGuard {
                _state: state,
                guard,
            });
        }
        Ok(ServerLifecycleWriteGuard {
            _state: state,
            guard: gate.write_owned().await,
        })
    }

    async fn lock_connect_gate_read(
        &self,
        operation: &'static str,
        server_name: &ServerName,
    ) -> anyhow::Result<ServerLifecycleReadGuard> {
        let state = self.server_state_for(server_name);
        let gate = Arc::clone(&state.lifecycle_gate);
        if let Some(guard) =
            self.fail_fast_if_reentrant(operation, || gate.clone().try_read_owned())?
        {
            return Ok(ServerLifecycleReadGuard {
                _state: state,
                guard,
            });
        }
        Ok(ServerLifecycleReadGuard {
            _state: state,
            guard: gate.read_owned().await,
        })
    }

    async fn wait_for_server_io_to_finish(
        &self,
        operation: &'static str,
        state: &ServerState,
    ) -> anyhow::Result<()> {
        if state.in_flight_io_count() == 0 {
            return Ok(());
        }

        if self.is_reentrant_handler_call() {
            return Err(tagged_message(
                ErrorKind::ManagerState,
                format!("{REENTRANT_HANDLER_ERROR}: {operation}"),
            ));
        }

        state.wait_for_in_flight_io().await;
        Ok(())
    }

    /// Inspect manager state under the shared lock without exposing borrowed data directly.
    pub async fn inspect<R>(&self, f: impl FnOnce(&Manager) -> R) -> crate::Result<R> {
        Ok(self
            .with_manager_lock("inspect", |manager| f(manager))
            .await?)
    }

    async fn try_prepare_connected_client(
        &self,
        operation: &'static str,
        server_name: &ServerName,
        cwd: Option<&Path>,
    ) -> anyhow::Result<Option<crate::manager::PreparedConnectedClient>> {
        self.lock_for_async_op(operation)
            .await?
            .try_prepare_connected_client(server_name.as_str(), cwd)
    }

    async fn try_prepare_reusable_connected_client(
        &self,
        operation: &'static str,
        config: &Config,
        server_name: &ServerName,
        cwd: Option<&Path>,
    ) -> anyhow::Result<Option<crate::manager::PreparedConnectedClient>> {
        let server_cfg = config.server_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::Config,
                format!("unknown mcp server: {server_name}"),
            )
        })?;
        self.lock_for_async_op(operation)
            .await?
            .try_prepare_reusable_connected_client(server_name.as_str(), server_cfg, cwd)
    }

    async fn ensure_connected_while_gated(
        &self,
        operation: &'static str,
        config: &Config,
        server_name: &ServerName,
        cwd: &Path,
    ) -> anyhow::Result<()> {
        let cwd = crate::manager::resolve_config_connection_cwd(config.thread_root(), cwd)?;

        if self
            .try_prepare_reusable_connected_client(operation, config, server_name, Some(&cwd))
            .await?
            .is_some()
        {
            return Ok(());
        }

        let lifecycle = {
            let mut manager = self.lock_for_async_op(operation).await?;
            manager
                .prepare_transport_connect(config, server_name.as_str(), &cwd)?
                .map(|prepared| manager.prepare_transport_lifecycle(prepared))
        };
        let Some(lifecycle) = lifecycle else {
            return Ok(());
        };

        let completed = lifecycle.run().await;
        self.lock_for_async_op(operation)
            .await?
            .finish_transport_lifecycle(completed)
    }

    async fn prepare_existing_connected_client_with_gate(
        &self,
        operation: &'static str,
        server_name: &ServerName,
    ) -> anyhow::Result<PreparedSharedClient> {
        let state = self.server_state_for(server_name);
        let gate = self.lock_connect_gate_read(operation, server_name).await?;
        let prepared = self
            .try_prepare_connected_client(operation, server_name, None)
            .await?
            .ok_or_else(|| {
                tagged_message(
                    ErrorKind::ManagerState,
                    format!("mcp server not connected: {server_name}"),
                )
            })?;
        let in_flight_io = state.start_in_flight_io();
        drop(gate);
        Ok(PreparedSharedClient {
            prepared,
            in_flight_io: Some(in_flight_io),
        })
    }

    async fn prepare_connected_client_with_gate(
        &self,
        operation: &'static str,
        config: &Config,
        server_name: &ServerName,
        cwd: &Path,
    ) -> anyhow::Result<PreparedSharedClient> {
        let cwd = crate::manager::resolve_config_connection_cwd(config.thread_root(), cwd)?;
        let state = self.server_state_for(server_name);
        let read_gate = self.lock_connect_gate_read(operation, server_name).await?;

        if let Some(prepared) = self
            .try_prepare_reusable_connected_client(operation, config, server_name, Some(&cwd))
            .await?
        {
            let in_flight_io = state.start_in_flight_io();
            drop(read_gate);
            return Ok(PreparedSharedClient {
                prepared,
                in_flight_io: Some(in_flight_io),
            });
        }
        drop(read_gate);

        let write_gate = self.lock_connect_gate_write(operation, server_name).await?;

        if let Some(prepared) = self
            .try_prepare_reusable_connected_client(operation, config, server_name, Some(&cwd))
            .await?
        {
            let in_flight_io = state.start_in_flight_io();
            drop(write_gate);
            return Ok(PreparedSharedClient {
                prepared,
                in_flight_io: Some(in_flight_io),
            });
        }

        self.ensure_connected_while_gated(operation, config, server_name, &cwd)
            .await?;

        let prepared = self
            .try_prepare_connected_client(operation, server_name, Some(&cwd))
            .await?
            .ok_or_else(|| {
                tagged_message(
                    ErrorKind::ManagerState,
                    format!("mcp server became unavailable before {operation}: {server_name}"),
                )
            })?;
        let in_flight_io = state.start_in_flight_io();
        drop(write_gate);
        Ok(PreparedSharedClient {
            prepared,
            in_flight_io: Some(in_flight_io),
        })
    }

    async fn cleanup_connection_after_error(&self, server_name: ServerName, connection_id: u64) {
        let state = self.server_state_for(&server_name);
        let _gate = match self
            .lock_connect_gate_write("cleanup_connection_after_error", &server_name)
            .await
        {
            Ok(guard) => guard,
            Err(_) => {
                self.spawn_connection_cleanup(server_name, connection_id);
                return;
            }
        };
        if self
            .wait_for_server_io_to_finish("cleanup_connection_after_error", state.as_ref())
            .await
            .is_err()
        {
            self.spawn_connection_cleanup(server_name, connection_id);
            return;
        }
        let disconnect = if self.is_reentrant_handler_call() {
            match self.inner.try_lock() {
                Ok(mut manager) => manager
                    .prepare_disconnect_for_wait_if_connection_with_cwd_cleanup(
                        server_name.as_str(),
                        connection_id,
                    ),
                Err(_) => {
                    self.spawn_connection_cleanup(server_name, connection_id);
                    return;
                }
            }
        } else {
            let mut manager = self.inner.lock().await;
            manager.prepare_disconnect_for_wait_if_connection_with_cwd_cleanup(
                server_name.as_str(),
                connection_id,
            )
        };
        disconnect.wait_for_jsonrpc_error_cleanup().await;
    }

    fn spawn_connection_cleanup(&self, server_name: ServerName, connection_id: u64) {
        let shared = self.clone();
        tokio::spawn(async move {
            let state = shared.server_state_for(&server_name);
            let _gate = match shared
                .lock_connect_gate_write("cleanup_connection_after_error", &server_name)
                .await
            {
                Ok(guard) => guard,
                Err(_) => return,
            };
            if shared
                .wait_for_server_io_to_finish("cleanup_connection_after_error", state.as_ref())
                .await
                .is_err()
            {
                return;
            }
            let disconnect = {
                let mut manager = shared.inner.lock().await;
                manager.prepare_disconnect_for_wait_if_connection_with_cwd_cleanup(
                    server_name.as_str(),
                    connection_id,
                )
            };
            disconnect.wait_for_jsonrpc_error_cleanup().await;
        });
    }

    async fn run_prepared_request(
        &self,
        prepared: PreparedSharedClient,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        let PreparedSharedClient {
            prepared,
            in_flight_io,
        } = prepared;
        let call = prepared.request_call(method, params).await;
        drop(in_flight_io);
        if call.cleanup().should_disconnect() {
            self.cleanup_connection_after_error(
                prepared.server_name.clone(),
                prepared.connection_id,
            )
            .await;
        }
        Ok(call.into_result()?)
    }

    async fn run_prepared_notify(
        &self,
        prepared: PreparedSharedClient,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        let PreparedSharedClient {
            prepared,
            in_flight_io,
        } = prepared;
        let call = prepared.notify_call(method, params).await;
        drop(in_flight_io);
        if call.cleanup().should_disconnect() {
            self.cleanup_connection_after_error(
                prepared.server_name.clone(),
                prepared.connection_id,
            )
            .await;
        }
        Ok(call.into_result()?)
    }

    pub async fn is_connected(&self, server_name: &str) -> crate::Result<bool> {
        self.is_connected_named(&Self::parse_server_name(server_name)?)
            .await
    }

    pub async fn is_connected_named(&self, server_name: &ServerName) -> crate::Result<bool> {
        Ok(self
            .with_manager_lock("is_connected_named", |manager| {
                manager.is_connected_named(server_name)
            })
            .await?)
    }

    pub async fn connected_server_names(&self) -> crate::Result<Vec<ServerName>> {
        Ok(self
            .with_manager_lock("connected_server_names", |manager| {
                manager.connected_server_names()
            })
            .await?)
    }

    pub async fn disconnect(&self, server_name: &str) -> crate::Result<bool> {
        self.disconnect_named(&Self::parse_server_name(server_name)?)
            .await
    }

    pub async fn disconnect_named(&self, server_name: &ServerName) -> crate::Result<bool> {
        let state = self.server_state_for(server_name);
        let _gate = self
            .lock_connect_gate_write("disconnect_named", server_name)
            .await?;
        self.wait_for_server_io_to_finish("disconnect_named", state.as_ref())
            .await?;
        Ok(self
            .with_manager_lock("disconnect_named", |manager| {
                manager.disconnect_named(server_name)
            })
            .await?)
    }

    pub async fn disconnect_and_wait(
        &self,
        server_name: &str,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        self.disconnect_and_wait_named(&Self::parse_server_name(server_name)?, timeout, on_timeout)
            .await
    }

    pub async fn disconnect_and_wait_named(
        &self,
        server_name: &ServerName,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        let state = self.server_state_for(server_name);
        let _gate = self
            .lock_connect_gate_write("disconnect_and_wait_named", server_name)
            .await?;
        self.wait_for_server_io_to_finish("disconnect_and_wait_named", state.as_ref())
            .await?;
        let disconnect = self
            .lock_for_async_op("disconnect_and_wait_named")
            .await?
            .prepare_disconnect_for_wait_with_cwd_cleanup(server_name.as_str());
        Ok(disconnect.wait_with_timeout(timeout, on_timeout).await?)
    }

    pub async fn request(
        &self,
        config: &Config,
        server_name: &str,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request_named(
            config,
            &Self::parse_server_name(server_name)?,
            method,
            params,
            cwd,
        )
        .await
    }

    pub async fn request_named(
        &self,
        config: &Config,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let prepared = self
            .prepare_connected_client_with_gate("request_named", config, server_name, cwd)
            .await?;
        self.run_prepared_request(prepared, method, params).await
    }

    pub async fn request_connected(
        &self,
        server_name: &str,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        self.request_connected_named(&Self::parse_server_name(server_name)?, method, params)
            .await
    }

    pub async fn request_connected_named(
        &self,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        let prepared = self
            .prepare_existing_connected_client_with_gate("request_connected_named", server_name)
            .await?;
        self.run_prepared_request(prepared, method, params).await
    }

    pub async fn request_typed<R: McpRequest>(
        &self,
        config: &Config,
        server_name: &str,
        params: Option<R::Params>,
        cwd: &Path,
    ) -> crate::Result<R::Result> {
        self.request_typed_named::<R>(config, &Self::parse_server_name(server_name)?, params, cwd)
            .await
    }

    pub async fn request_typed_named<R: McpRequest>(
        &self,
        config: &Config,
        server_name: &ServerName,
        params: Option<R::Params>,
        cwd: &Path,
    ) -> crate::Result<R::Result> {
        let params = crate::mcp::serialize_request_params::<R>(server_name.as_str(), params)?;
        let result = self
            .request_named(config, server_name, R::METHOD, params, cwd)
            .await?;
        crate::mcp::deserialize_request_result::<R>(server_name.as_str(), result)
    }

    pub async fn request_typed_connected<R: McpRequest>(
        &self,
        server_name: &str,
        params: Option<R::Params>,
    ) -> crate::Result<R::Result> {
        self.request_typed_connected_named::<R>(&Self::parse_server_name(server_name)?, params)
            .await
    }

    pub async fn request_typed_connected_named<R: McpRequest>(
        &self,
        server_name: &ServerName,
        params: Option<R::Params>,
    ) -> crate::Result<R::Result> {
        let params = crate::mcp::serialize_request_params::<R>(server_name.as_str(), params)?;
        let result = self
            .request_connected_named(server_name, R::METHOD, params)
            .await?;
        crate::mcp::deserialize_request_result::<R>(server_name.as_str(), result)
    }

    pub async fn notify(
        &self,
        config: &Config,
        server_name: &str,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<()> {
        self.notify_named(
            config,
            &Self::parse_server_name(server_name)?,
            method,
            params,
            cwd,
        )
        .await
    }

    pub async fn notify_named(
        &self,
        config: &Config,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<()> {
        let prepared = self
            .prepare_connected_client_with_gate("notify_named", config, server_name, cwd)
            .await?;
        self.run_prepared_notify(prepared, method, params).await
    }

    pub async fn notify_connected(
        &self,
        server_name: &str,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        self.notify_connected_named(&Self::parse_server_name(server_name)?, method, params)
            .await
    }

    pub async fn notify_connected_named(
        &self,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        let prepared = self
            .prepare_existing_connected_client_with_gate("notify_connected_named", server_name)
            .await?;
        self.run_prepared_notify(prepared, method, params).await
    }

    pub async fn notify_typed<N: McpNotification>(
        &self,
        config: &Config,
        server_name: &str,
        params: Option<N::Params>,
        cwd: &Path,
    ) -> crate::Result<()> {
        self.notify_typed_named::<N>(config, &Self::parse_server_name(server_name)?, params, cwd)
            .await
    }

    pub async fn notify_typed_named<N: McpNotification>(
        &self,
        config: &Config,
        server_name: &ServerName,
        params: Option<N::Params>,
        cwd: &Path,
    ) -> crate::Result<()> {
        let params = crate::mcp::serialize_notification_params::<N>(server_name.as_str(), params)?;
        self.notify_named(config, server_name, N::METHOD, params, cwd)
            .await
    }

    pub async fn notify_typed_connected<N: McpNotification>(
        &self,
        server_name: &str,
        params: Option<N::Params>,
    ) -> crate::Result<()> {
        self.notify_typed_connected_named::<N>(&Self::parse_server_name(server_name)?, params)
            .await
    }

    pub async fn notify_typed_connected_named<N: McpNotification>(
        &self,
        server_name: &ServerName,
        params: Option<N::Params>,
    ) -> crate::Result<()> {
        let params = crate::mcp::serialize_notification_params::<N>(server_name.as_str(), params)?;
        self.notify_connected_named(server_name, N::METHOD, params)
            .await
    }

    pub async fn server_handler_timeout_count(&self, server_name: &str) -> crate::Result<u64> {
        self.server_handler_timeout_count_named(&Self::parse_server_name(server_name)?)
            .await
    }

    pub async fn server_handler_timeout_count_named(
        &self,
        server_name: &ServerName,
    ) -> crate::Result<u64> {
        Ok(self
            .with_manager_lock("server_handler_timeout_count_named", |manager| {
                manager.server_handler_timeout_count(server_name.as_str())
            })
            .await?)
    }

    /// Returns a snapshot of timeout counts for all servers without draining shared state.
    pub async fn server_handler_timeout_counts(&self) -> crate::Result<HashMap<ServerName, u64>> {
        Ok(self
            .with_manager_lock("server_handler_timeout_counts", |manager| {
                manager.server_handler_timeout_counts()
            })
            .await?)
    }

    /// Takes timeout counts for all servers and resets the shared counters for every clone.
    pub async fn take_server_handler_timeout_counts(
        &self,
    ) -> crate::Result<HashMap<ServerName, u64>> {
        Ok(self
            .with_manager_lock("take_server_handler_timeout_counts", |manager| {
                manager.take_server_handler_timeout_counts()
            })
            .await?)
    }

    /// Returns recorded protocol-version mismatches without draining shared state.
    pub async fn protocol_version_mismatches(&self) -> crate::Result<Vec<ProtocolVersionMismatch>> {
        Ok(self
            .with_manager_lock("protocol_version_mismatches", |manager| {
                manager.protocol_version_mismatches().to_vec()
            })
            .await?)
    }

    /// Takes recorded protocol-version mismatches and clears them for every clone.
    pub async fn take_protocol_version_mismatches(
        &self,
    ) -> crate::Result<Vec<ProtocolVersionMismatch>> {
        Ok(self
            .with_manager_lock("take_protocol_version_mismatches", |manager| {
                manager.take_protocol_version_mismatches()
            })
            .await?)
    }
}

impl Clone for SharedManager {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            server_states: Arc::clone(&self.server_states),
            manager_id: self.manager_id,
            captured_handler_scope: self
                .captured_handler_scope
                .clone()
                .or_else(|| crate::manager::current_manager_handler_scope_token(self.manager_id)),
        }
    }
}

struct PreparedSharedClient {
    prepared: crate::manager::PreparedConnectedClient,
    in_flight_io: Option<InFlightIoGuard>,
}

struct ServerLifecycleReadGuard {
    _state: Arc<ServerState>,
    guard: OwnedRwLockReadGuard<()>,
}

struct ServerLifecycleWriteGuard {
    _state: Arc<ServerState>,
    guard: OwnedRwLockWriteGuard<()>,
}

#[derive(Default)]
struct ServerState {
    lifecycle_gate: Arc<RwLock<()>>,
    in_flight_io: AtomicUsize,
    in_flight_idle: tokio::sync::Notify,
}

impl ServerState {
    fn start_in_flight_io(self: &Arc<Self>) -> InFlightIoGuard {
        self.in_flight_io.fetch_add(1, AtomicOrdering::AcqRel);
        InFlightIoGuard {
            state: Some(Arc::clone(self)),
        }
    }

    fn in_flight_io_count(&self) -> usize {
        self.in_flight_io.load(AtomicOrdering::Acquire)
    }

    async fn wait_for_in_flight_io(&self) {
        self.wait_for_in_flight_io_with_hook(|| {}).await;
    }

    async fn wait_for_in_flight_io_with_hook(&self, mut after_waiter_registration: impl FnMut()) {
        loop {
            let notified = self.in_flight_idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            after_waiter_registration();
            if self.in_flight_io_count() == 0 {
                return;
            }
            notified.await;
        }
    }
}

struct InFlightIoGuard {
    state: Option<Arc<ServerState>>,
}

impl Drop for InFlightIoGuard {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        if state.in_flight_io.fetch_sub(1, AtomicOrdering::AcqRel) == 1 {
            state.in_flight_idle.notify_waiters();
        }
    }
}

impl Drop for ServerLifecycleReadGuard {
    fn drop(&mut self) {
        let _ = &self.guard;
    }
}

impl Drop for ServerLifecycleWriteGuard {
    fn drop(&mut self) {
        let _ = &self.guard;
    }
}

#[cfg(test)]
mod tests {
    use super::ServerState;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    #[cfg(unix)]
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    #[cfg(not(windows))]
    use crate::test_support::{CurrentDirRestoreGuard, cwd_test_guard_async};
    use crate::{
        ClientConfig, Config, Manager, McpRequest, ProtocolVersionCheck, ServerConfig, ServerName,
        ServerRequestHandler, ServerRequestOutcome, SharedManager, TrustMode,
    };

    struct NestedRequest;

    #[derive(Serialize)]
    struct NestedParams {
        phase: &'static str,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
    struct NestedResult {
        nested: bool,
    }

    impl McpRequest for NestedRequest {
        const METHOD: &'static str = "nested";
        type Params = NestedParams;
        type Result = NestedResult;
    }

    fn absolute_test_cwd() -> &'static Path {
        static CWD: OnceLock<PathBuf> = OnceLock::new();
        CWD.get_or_init(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .expect("mcp-kit tests require a stable workspace root")
                .to_path_buf()
        })
        .as_path()
    }

    fn test_workspace_path(name: &str) -> PathBuf {
        absolute_test_cwd().join("workspace").join(name)
    }

    #[derive(Clone, Copy, Debug)]
    enum SharedRequestPath {
        ConfigDriven,
        ConfigDrivenNamed,
        Connected,
        ConnectedNamed,
    }

    impl SharedRequestPath {
        const fn label(self) -> &'static str {
            match self {
                Self::ConfigDriven => "config-driven",
                Self::ConfigDrivenNamed => "config-driven-named",
                Self::Connected => "connected",
                Self::ConnectedNamed => "connected-named",
            }
        }
    }

    fn record_reusable_connection(
        manager: &mut Manager,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) {
        manager.record_connection_cwd(server_name, cwd).unwrap();
        manager
            .record_connection_server_config_effective_with_base(
                server_name,
                config.server(server_name).unwrap(),
                cwd,
                None,
                Some(cwd),
            )
            .unwrap();
    }

    async fn shared_request_via_path(
        path: SharedRequestPath,
        shared: SharedManager,
        config: Arc<Config>,
        cwd: PathBuf,
    ) -> crate::Result<Value> {
        let server_name = ServerName::parse("srv").unwrap();
        match path {
            SharedRequestPath::ConfigDriven => {
                shared
                    .request(config.as_ref(), "srv", "ping", None::<Value>, &cwd)
                    .await
            }
            SharedRequestPath::ConfigDrivenNamed => {
                shared
                    .request_named(config.as_ref(), &server_name, "ping", None::<Value>, &cwd)
                    .await
            }
            SharedRequestPath::Connected => shared.request_connected("srv", "ping", None).await,
            SharedRequestPath::ConnectedNamed => {
                shared
                    .request_connected_named(&server_name, "ping", None::<Value>)
                    .await
            }
        }
    }

    #[tokio::test]
    async fn shared_manager_clones_share_cache() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        let clone = shared.clone();
        assert_eq!(shared.connected_server_names().await.unwrap().len(), 1);
        let result = clone.request_connected("srv", "ping", None).await.unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn server_state_wait_for_in_flight_io_does_not_miss_final_drop_notification() {
        let state = Arc::new(ServerState::default());
        let guard = StdMutex::new(Some(state.start_in_flight_io()));

        let wait_result = tokio::time::timeout(
            Duration::from_secs(1),
            state.wait_for_in_flight_io_with_hook(|| {
                drop(guard.lock().unwrap().take());
            }),
        )
        .await;
        wait_result.expect(
            "wait_for_in_flight_io should not hang when the final in-flight guard drops after waiter registration",
        );

        assert_eq!(state.in_flight_io_count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_cold_start_requests_can_overlap_on_same_server() {
        let Some(socket_path) = unique_socket_path(
            "shared_manager_cold_start_requests_can_overlap_on_same_server",
            "shared-overlap",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let first_request_line = lines.next_line().await.unwrap().unwrap();
            let first_request: Value = serde_json::from_str(&first_request_line).unwrap();
            assert_eq!(first_request["method"], "ping");

            let second_request_line =
                tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                    .await
                    .expect("second request should arrive before the first response is sent")
                    .unwrap()
                    .unwrap();
            let second_request: Value = serde_json::from_str(&second_request_line).unwrap();
            assert_eq!(second_request["method"], "ping");

            for request in [&first_request, &second_request] {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"].clone(),
                    "result": { "ok": true },
                });
                let mut response_line = serde_json::to_string(&response).unwrap();
                response_line.push('\n');
                server_write
                    .write_all(response_line.as_bytes())
                    .await
                    .unwrap();
                server_write.flush().await.unwrap();
            }
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Config::new(ClientConfig::default(), servers);

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();
        let clone = shared.clone();

        let (first, second) = tokio::join!(
            shared.request(&config, "srv", "ping", None, Path::new("/")),
            clone.request(&config, "srv", "ping", None, Path::new("/")),
        );
        assert_eq!(first.unwrap(), serde_json::json!({ "ok": true }));
        assert_eq!(second.unwrap(), serde_json::json!({ "ok": true }));

        server_task.await.unwrap();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn shared_manager_request_paths_overlap_on_same_connected_server() {
        for path in [
            SharedRequestPath::ConfigDriven,
            SharedRequestPath::ConfigDrivenNamed,
            SharedRequestPath::Connected,
            SharedRequestPath::ConnectedNamed,
        ] {
            let (client_stream, server_stream) = tokio::io::duplex(1024);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let server_task = tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(server_read).lines();

                let init_line = lines.next_line().await.unwrap().unwrap();
                let init_value: Value = serde_json::from_str(&init_line).unwrap();
                assert_eq!(init_value["method"], "initialize");
                let init_id = init_value["id"].clone();

                let init_response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": init_id,
                    "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
                });
                let mut init_response_line = serde_json::to_string(&init_response).unwrap();
                init_response_line.push('\n');
                server_write
                    .write_all(init_response_line.as_bytes())
                    .await
                    .unwrap();
                server_write.flush().await.unwrap();

                let initialized_line = lines.next_line().await.unwrap().unwrap();
                let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
                assert_eq!(initialized_value["method"], "notifications/initialized");

                let first_request_line = lines.next_line().await.unwrap().unwrap();
                let first_request: Value = serde_json::from_str(&first_request_line).unwrap();
                assert_eq!(first_request["method"], "ping");

                let second_request_line =
                    tokio::time::timeout(Duration::from_millis(200), lines.next_line())
                        .await
                        .expect("second request should overlap before the first response")
                        .unwrap()
                        .unwrap();
                let second_request: Value = serde_json::from_str(&second_request_line).unwrap();
                assert_eq!(second_request["method"], "ping");

                for request in [&first_request, &second_request] {
                    let response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request["id"].clone(),
                        "result": { "ok": true },
                    });
                    let mut response_line = serde_json::to_string(&response).unwrap();
                    response_line.push('\n');
                    server_write
                        .write_all(response_line.as_bytes())
                        .await
                        .unwrap();
                    server_write.flush().await.unwrap();
                }
            });

            let cwd = test_workspace_path(path.label());
            let mut servers = BTreeMap::new();
            servers.insert(
                ServerName::parse("srv").unwrap(),
                ServerConfig::stdio(vec!["mock-server".to_string()]).unwrap(),
            );
            let config = Arc::new(Config::new(ClientConfig::default(), servers));

            let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
                .with_trust_mode(TrustMode::Trusted);
            manager
                .connect_io("srv", client_read, client_write)
                .await
                .unwrap();
            record_reusable_connection(&mut manager, config.as_ref(), "srv", &cwd);

            let shared = manager.into_shared();
            let (first, second) = tokio::join!(
                shared_request_via_path(path, shared.clone(), Arc::clone(&config), cwd.clone()),
                shared_request_via_path(path, shared.clone(), Arc::clone(&config), cwd.clone()),
            );
            assert_eq!(first.unwrap(), serde_json::json!({ "ok": true }));
            assert_eq!(second.unwrap(), serde_json::json!({ "ok": true }));

            server_task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn shared_manager_request_connected_tracks_in_flight_io_until_completion() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let (request_seen_tx, request_seen_rx) = oneshot::channel();
        let (release_response_tx, release_response_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();
            request_seen_tx.send(()).unwrap();
            release_response_rx.await.unwrap();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(
                eof.is_none(),
                "disconnect should close after request_connected finishes"
            );
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();
        let shared = manager.into_shared();

        let request_task = tokio::spawn({
            let shared = shared.clone();
            async move { shared.request_connected("srv", "ping", None).await }
        });
        request_seen_rx.await.unwrap();

        let disconnect_task = tokio::spawn({
            let shared = shared.clone();
            async move { shared.disconnect("srv").await }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !disconnect_task.is_finished(),
            "disconnect should wait while request_connected still owns the server read gate"
        );

        release_response_tx.send(()).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(1), request_task)
            .await
            .expect("request_connected should finish after the delayed response")
            .unwrap()
            .unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let disconnected = tokio::time::timeout(Duration::from_secs(1), disconnect_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            disconnected,
            "disconnect should observe the connected server once the request finishes"
        );

        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server should observe request then disconnect")
            .unwrap();
    }

    #[tokio::test]
    async fn shared_manager_disconnect_waits_for_in_flight_request_across_request_paths() {
        for path in [
            SharedRequestPath::ConfigDriven,
            SharedRequestPath::ConfigDrivenNamed,
            SharedRequestPath::Connected,
            SharedRequestPath::ConnectedNamed,
        ] {
            let (client_stream, server_stream) = tokio::io::duplex(1024);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);
            let (request_seen_tx, request_seen_rx) = oneshot::channel();
            let (release_response_tx, release_response_rx) = oneshot::channel();

            let server_task = tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(server_read).lines();

                let init_line = lines.next_line().await.unwrap().unwrap();
                let init_value: Value = serde_json::from_str(&init_line).unwrap();
                assert_eq!(init_value["method"], "initialize");
                let init_id = init_value["id"].clone();

                let init_response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": init_id,
                    "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
                });
                let mut init_response_line = serde_json::to_string(&init_response).unwrap();
                init_response_line.push('\n');
                server_write
                    .write_all(init_response_line.as_bytes())
                    .await
                    .unwrap();
                server_write.flush().await.unwrap();

                let initialized_line = lines.next_line().await.unwrap().unwrap();
                let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
                assert_eq!(initialized_value["method"], "notifications/initialized");

                let request_line = lines.next_line().await.unwrap().unwrap();
                let request_value: Value = serde_json::from_str(&request_line).unwrap();
                assert_eq!(request_value["method"], "ping");
                let request_id = request_value["id"].clone();
                request_seen_tx.send(()).unwrap();
                release_response_rx.await.unwrap();

                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": { "ok": true },
                });
                let mut response_line = serde_json::to_string(&response).unwrap();
                response_line.push('\n');
                server_write
                    .write_all(response_line.as_bytes())
                    .await
                    .unwrap();
                server_write.flush().await.unwrap();

                let eof = lines.next_line().await.unwrap();
                assert!(
                    eof.is_none(),
                    "disconnect should close after request finishes"
                );
            });

            let cwd = test_workspace_path(path.label());
            let mut servers = BTreeMap::new();
            servers.insert(
                ServerName::parse("srv").unwrap(),
                ServerConfig::stdio(vec!["mock-server".to_string()]).unwrap(),
            );
            let config = Arc::new(Config::new(ClientConfig::default(), servers));

            let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
                .with_trust_mode(TrustMode::Trusted);
            manager
                .connect_io("srv", client_read, client_write)
                .await
                .unwrap();
            record_reusable_connection(&mut manager, config.as_ref(), "srv", &cwd);
            let shared = manager.into_shared();

            let request_task = tokio::spawn(shared_request_via_path(
                path,
                shared.clone(),
                Arc::clone(&config),
                cwd.clone(),
            ));
            request_seen_rx.await.unwrap();

            let disconnect_task = tokio::spawn({
                let shared = shared.clone();
                async move { shared.disconnect("srv").await }
            });

            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(
                !disconnect_task.is_finished(),
                "{} disconnect should wait for in-flight request IO",
                path.label()
            );

            release_response_tx.send(()).unwrap();

            let result = tokio::time::timeout(Duration::from_secs(1), request_task)
                .await
                .expect("request should finish after releasing response")
                .unwrap()
                .unwrap();
            assert_eq!(result, serde_json::json!({ "ok": true }));

            let disconnected = tokio::time::timeout(Duration::from_secs(1), disconnect_task)
                .await
                .expect("disconnect should finish after request drains")
                .unwrap()
                .unwrap();
            assert!(
                disconnected,
                "{} disconnect should remove the server after in-flight request completes",
                path.label()
            );

            tokio::time::timeout(Duration::from_secs(1), server_task)
                .await
                .expect("server should observe request then disconnect")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn shared_manager_inspect_reads_initialize_result() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        let server_name = shared
            .inspect(|manager| {
                manager
                    .initialize_result("srv")
                    .and_then(|value| value["serverInfo"]["name"].as_str())
                    .map(str::to_string)
            })
            .await
            .unwrap();
        assert_eq!(server_name.as_deref(), Some("demo"));

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_disconnect_affects_all_clones() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after disconnect");
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();
        let shared = manager.into_shared();
        let clone = shared.clone();

        assert!(shared.disconnect("srv").await.unwrap());
        assert!(!clone.is_connected("srv").await.unwrap());

        server_task.abort();
    }

    #[tokio::test]
    async fn shared_manager_spawned_handler_task_fails_fast_like_direct_reentrancy() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let (send_notification_tx, send_notification_rx) = oneshot::channel();
        let (handler_result_tx, handler_result_rx) = oneshot::channel();
        let shared_slot = Arc::new(StdMutex::new(None::<SharedManager>));
        let shared_slot_for_handler = Arc::clone(&shared_slot);
        let handler_result_tx = Arc::new(StdMutex::new(Some(handler_result_tx)));
        let handler_result_tx_for_handler = Arc::clone(&handler_result_tx);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            send_notification_rx.await.unwrap();

            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "demo/notify",
                "params": {},
            });
            let mut notification_line = serde_json::to_string(&notification).unwrap();
            notification_line.push('\n');
            server_write
                .write_all(notification_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after test completes");
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_notification_handler(Arc::new(move |_ctx| {
                let shared_slot_for_handler = Arc::clone(&shared_slot_for_handler);
                let handler_result_tx_for_handler = Arc::clone(&handler_result_tx_for_handler);
                Box::pin(async move {
                    let shared = shared_slot_for_handler
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .expect("shared manager should be installed before notification")
                        .clone();
                    let child = tokio::spawn(async move { shared.inspect(|_| ()).await });
                    let message = match child.await {
                        Ok(Ok(())) => "spawned shared-manager call succeeded".to_string(),
                        Ok(Err(err)) => format!("{err:#}"),
                        Err(err) => panic!("spawned task should join: {err}"),
                    };
                    if let Some(tx) = handler_result_tx_for_handler
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        let _ = tx.send(message);
                    }
                })
            }));
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        shared_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(shared.clone());

        let held_lock = shared.lock_for_async_op("held_lock").await.unwrap();
        send_notification_tx.send(()).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(1), handler_result_rx)
            .await
            .expect("spawned handler task should fail fast instead of waiting")
            .unwrap();
        assert!(
            result.contains(super::REENTRANT_HANDLER_ERROR),
            "unexpected result: {result}"
        );
        assert!(
            result.contains("inspect"),
            "spawned task error should identify the blocked operation: {result}"
        );

        drop(held_lock);
        shared_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(shared);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_external_call_waits_even_if_other_handler_scope_is_active() {
        let manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        let active_handler_scopes = manager.active_handler_scopes();
        active_handler_scopes.fetch_add(1, Ordering::Relaxed);

        let shared = manager.into_shared();
        let (release_tx, release_rx) = oneshot::channel();
        let held_lock = tokio::spawn({
            let shared = shared.clone();
            async move {
                let guard = shared.lock_for_async_op("held_lock").await.unwrap();
                let _ = release_rx.await;
                drop(guard);
            }
        });

        tokio::task::yield_now().await;

        let inspect = tokio::spawn({
            let shared = shared.clone();
            async move { shared.inspect(|_| ()).await }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !inspect.is_finished(),
            "external call should wait instead of failing fast"
        );

        release_tx.send(()).unwrap();
        held_lock.await.unwrap();
        inspect.await.unwrap().unwrap();
        active_handler_scopes.fetch_sub(1, Ordering::Relaxed);
    }

    #[tokio::test]
    async fn shared_manager_external_connect_gate_waits_even_if_other_handler_scope_is_active() {
        let manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        let active_handler_scopes = manager.active_handler_scopes();
        active_handler_scopes.fetch_add(1, Ordering::Relaxed);

        let shared = manager.into_shared();
        let server_name = ServerName::parse("srv").unwrap();
        let (release_tx, release_rx) = oneshot::channel();
        let held_gate = tokio::spawn({
            let shared = shared.clone();
            let server_name = server_name.clone();
            async move {
                let guard = shared
                    .lock_connect_gate_write("held_gate", &server_name)
                    .await
                    .unwrap();
                let _ = release_rx.await;
                drop(guard);
            }
        });

        tokio::task::yield_now().await;

        let wait_for_gate = tokio::spawn({
            let shared = shared.clone();
            let server_name = server_name.clone();
            async move {
                shared
                    .lock_connect_gate_write("second_gate", &server_name)
                    .await
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !wait_for_gate.is_finished(),
            "external call should wait on the server gate instead of failing fast"
        );

        release_tx.send(()).unwrap();
        held_gate.await.unwrap();
        drop(wait_for_gate.await.unwrap().unwrap());
        active_handler_scopes.fetch_sub(1, Ordering::Relaxed);
    }

    #[tokio::test]
    async fn shared_manager_request_rejects_different_cwd_context() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after test completes");
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();
        let connected_cwd = test_workspace_path("a");
        let requested_cwd = test_workspace_path("b");
        manager
            .record_connection_cwd("srv", &connected_cwd)
            .unwrap();

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::stdio(vec!["mock-server".to_string()]).unwrap(),
        );
        let shared = manager.into_shared();
        let config = Config::new(ClientConfig::default(), servers);
        let err = shared
            .request(&config, "srv", "ping", None, &requested_cwd)
            .await
            .expect_err("different cwd should be rejected");
        assert!(
            err.to_string().contains("cannot be reused for cwd="),
            "{err:#}"
        );
        assert!(shared.is_connected("srv").await.unwrap());
        drop(shared);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_stale_cleanup_does_not_disconnect_replacement_connection() {
        let (old_client_stream, old_server_stream) = tokio::io::duplex(1024);
        let (old_client_read, old_client_write) = tokio::io::split(old_client_stream);
        let (old_server_read, mut old_server_write) = tokio::io::split(old_server_stream);

        let old_server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(old_server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "old" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            old_server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            old_server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let eof = lines.next_line().await.unwrap();
            assert!(
                eof.is_none(),
                "old connection should be closed during replacement"
            );
        });

        let (new_client_stream, new_server_stream) = tokio::io::duplex(1024);
        let (new_client_read, new_client_write) = tokio::io::split(new_client_stream);
        let (new_server_read, mut new_server_write) = tokio::io::split(new_server_stream);

        let new_server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(new_server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "new" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            new_server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            new_server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            new_server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            new_server_write.flush().await.unwrap();
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", old_client_read, old_client_write)
            .await
            .unwrap();
        let shared = manager.into_shared();
        let server_name = ServerName::parse("srv").unwrap();

        let prepared = shared
            .try_prepare_connected_client("stale_cleanup_prepare", &server_name, None)
            .await
            .unwrap()
            .expect("prepared old connection");

        {
            let mut manager = shared
                .lock_for_async_op("replace_connection")
                .await
                .unwrap();
            assert!(manager.disconnect("srv"));
            manager
                .connect_io("srv", new_client_read, new_client_write)
                .await
                .unwrap();
        }

        let disconnect = {
            let mut manager = shared.lock_for_async_op("stale_cleanup").await.unwrap();
            manager.prepare_disconnect_for_wait_if_connection(
                prepared.server_name.as_str(),
                prepared.connection_id,
            )
        };
        disconnect.wait_for_jsonrpc_error_cleanup().await;

        assert!(
            shared.is_connected("srv").await.unwrap(),
            "stale cleanup should not remove the replacement connection"
        );
        let result = shared.request_connected("srv", "ping", None).await.unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        old_server_task.await.unwrap();
        new_server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_disconnect_and_wait_releases_lock_while_waiting() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let notify = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "demo/notify",
                "params": {},
            });
            let mut notify_line = serde_json::to_string(&notify).unwrap();
            notify_line.push('\n');
            server_write
                .write_all(notify_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after disconnect");
        });

        let handler_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started_for_handler = Arc::clone(&handler_started);
        let handler_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped_for_handler = Arc::clone(&handler_dropped);

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_notification_handler(Arc::new(move |_ctx| {
                let started_for_handler = Arc::clone(&started_for_handler);
                let dropped_for_handler = Arc::clone(&dropped_for_handler);
                Box::pin(async move {
                    struct OnDrop(std::sync::Arc<std::sync::atomic::AtomicBool>);

                    impl Drop for OnDrop {
                        fn drop(&mut self) {
                            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }

                    let _on_drop = OnDrop(dropped_for_handler);
                    started_for_handler.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::future::pending::<()>().await;
                })
            }));
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handler_started.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let shared = manager.into_shared();
        let clone = shared.clone();
        let disconnect_task = tokio::spawn(async move {
            shared
                .disconnect_and_wait(
                    "srv",
                    Duration::from_millis(100),
                    mcp_jsonrpc::WaitOnTimeout::ReturnError,
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        let names = tokio::time::timeout(Duration::from_millis(20), clone.connected_server_names())
            .await
            .expect("connected_server_names should not be blocked by disconnect_and_wait")
            .unwrap();
        assert!(
            names.is_empty(),
            "disconnect should remove the shared connection early"
        );

        let err = disconnect_task.await.unwrap().unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("wait timed out after"),
            "unexpected error: {err_chain}"
        );

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handler_dropped.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        server_task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_disconnect_and_wait_blocks_same_server_reconnect_until_wait_finishes() {
        use tokio::sync::oneshot;

        let Some(socket_path) = unique_socket_path(
            "shared_manager_disconnect_and_wait_blocks_same_server_reconnect_until_wait_finishes",
            "disconnect-wait-gate",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let (notify_started_tx, notify_started_rx) = oneshot::channel();
        let (second_accept_seen_tx, second_accept_seen_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let notify = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "demo/notify",
                "params": {},
            });
            let mut notify_line = serde_json::to_string(&notify).unwrap();
            notify_line.push('\n');
            server_write
                .write_all(notify_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
            let _ = notify_started_tx.send(());

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after disconnect_and_wait");

            let (stream, _) = listener.accept().await.unwrap();
            let _ = second_accept_seen_tx.send(());
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo-reconnect" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after reconnect request");
        });

        let handler_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started_for_handler = Arc::clone(&handler_started);
        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Config::new(ClientConfig::default(), servers);

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_notification_handler(Arc::new(move |_ctx| {
                let started_for_handler = Arc::clone(&started_for_handler);
                Box::pin(async move {
                    started_for_handler.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::future::pending::<()>().await;
                })
            }));
        manager
            .get_or_connect(&config, "srv", Path::new("/"))
            .await
            .unwrap();
        notify_started_rx.await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handler_started.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let shared = manager.into_shared();
        let disconnect_shared = shared.clone();
        let disconnect_task = tokio::spawn(async move {
            disconnect_shared
                .disconnect_and_wait(
                    "srv",
                    Duration::from_millis(100),
                    mcp_jsonrpc::WaitOnTimeout::ReturnError,
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        let reconnect_shared = shared.clone();
        let reconnect_task = tokio::spawn(async move {
            reconnect_shared
                .request(&config, "srv", "ping", None::<Value>, Path::new("/"))
                .await
        });

        let mut second_accept_seen_rx = second_accept_seen_rx;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second_accept_seen_rx)
                .await
                .is_err(),
            "same-server reconnect should stay blocked until disconnect_and_wait returns"
        );

        let disconnect_err = disconnect_task.await.unwrap().unwrap_err();
        let disconnect_err_chain = format!("{disconnect_err:#}");
        assert!(
            disconnect_err_chain.contains("wait timed out after"),
            "{disconnect_err_chain}"
        );

        second_accept_seen_rx.await.unwrap();
        let reconnect_result = tokio::time::timeout(Duration::from_secs(1), reconnect_task)
            .await
            .expect("reconnect request should finish after disconnect_and_wait returns")
            .unwrap()
            .unwrap();
        assert_eq!(reconnect_result, serde_json::json!({ "ok": true }));

        assert!(shared.disconnect("srv").await.unwrap());
        server_task.await.unwrap();
        let _ = std::fs::remove_file(socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_disconnect_waits_for_inflight_connect_and_prevents_revival() {
        use tokio::sync::oneshot;

        let Some(socket_path) = unique_socket_path(
            "shared_manager_disconnect_waits_for_inflight_connect_and_prevents_revival",
            "disconnect-race",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let (init_seen_tx, init_seen_rx) = oneshot::channel();
        let (release_init_tx, release_init_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();
            let _ = init_seen_tx.send(());

            release_init_rx.await.unwrap();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            match tokio::time::timeout(Duration::from_millis(200), lines.next_line()).await {
                Ok(Ok(Some(request_line))) => {
                    let request_value: Value = serde_json::from_str(&request_line).unwrap();
                    assert_eq!(request_value["method"], "ping");
                    let request_id = request_value["id"].clone();

                    let response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": { "ok": true },
                    });
                    let mut response_line = serde_json::to_string(&response).unwrap();
                    response_line.push('\n');
                    if server_write
                        .write_all(response_line.as_bytes())
                        .await
                        .is_ok()
                        && server_write.flush().await.is_ok()
                    {
                        let eof = lines.next_line().await.unwrap();
                        assert!(eof.is_none(), "expected EOF after disconnect");
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(err)) => panic!("read post-initialize line: {err}"),
                Err(_) => panic!("expected shared request to either send ping or close"),
            }
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Arc::new(Config::new(ClientConfig::default(), servers));

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();

        let request_shared = shared.clone();
        let request_config = Arc::clone(&config);
        let request_task = tokio::spawn(async move {
            request_shared
                .request(
                    request_config.as_ref(),
                    "srv",
                    "ping",
                    None::<Value>,
                    Path::new("/"),
                )
                .await
        });

        init_seen_rx.await.unwrap();

        let disconnect_shared = shared.clone();
        let mut disconnect_task =
            tokio::spawn(async move { disconnect_shared.disconnect("srv").await });

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut disconnect_task)
                .await
                .is_err(),
            "disconnect should wait for the in-flight connect gate"
        );

        release_init_tx.send(()).unwrap();

        let disconnected = tokio::time::timeout(Duration::from_secs(1), &mut disconnect_task)
            .await
            .expect("disconnect should finish after initialize completes")
            .unwrap()
            .unwrap();
        assert!(
            disconnected,
            "disconnect should remove the connection after the in-flight connect commits"
        );
        assert!(!shared.is_connected("srv").await.unwrap());
        assert!(shared.connected_server_names().await.unwrap().is_empty());
        assert!(shared.request_connected("srv", "ping", None).await.is_err());

        let _ = tokio::time::timeout(Duration::from_secs(1), request_task)
            .await
            .expect("outer request should not hang");

        server_task.await.unwrap();
        let _ = std::fs::remove_file(socket_path);
    }

    #[test]
    fn shared_manager_try_unwrap_requires_unique_owner() {
        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(1)).into_shared();
        let clone = shared.clone();
        assert!(shared.clone().try_unwrap().is_err());
        drop(clone);
        let inner = match shared.try_unwrap() {
            Ok(inner) => inner,
            Err(_) => panic!("unique owner should unwrap"),
        };
        assert_eq!(inner.trust_mode(), TrustMode::Untrusted);
    }

    #[test]
    fn shared_manager_server_states_prune_stale_entries() {
        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(1)).into_shared();
        let alpha = ServerName::parse(" alpha ").unwrap();
        let beta = ServerName::parse("beta").unwrap();

        let first = shared.server_state_for(&alpha);
        {
            let states = shared
                .server_states
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(states.len(), 1);
            assert_eq!(states.get(&alpha).unwrap().strong_count(), 1);
        }
        drop(first);

        let second = shared.server_state_for(&beta);
        {
            let states = shared
                .server_states
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(states.len(), 1);
            assert!(states.get("alpha").is_none());
            assert_eq!(states.get(&beta).unwrap().strong_count(), 1);
        }
        drop(second);
    }

    #[tokio::test]
    async fn shared_manager_telemetry_drain_is_shared() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": "1900-01-01" },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(1))
            .with_trust_mode(TrustMode::Trusted)
            .with_protocol_version_check(ProtocolVersionCheck::Warn);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        let clone = shared.clone();

        let snapshot = shared.protocol_version_mismatches().await.unwrap();
        assert_eq!(snapshot.len(), 1);
        let still_present = clone
            .inspect(|manager| manager.protocol_version_mismatches().len())
            .await
            .unwrap();
        assert_eq!(still_present, 1);

        let mismatches = shared.take_protocol_version_mismatches().await.unwrap();
        assert_eq!(mismatches.len(), 1);
        let after = clone
            .inspect(|manager| manager.protocol_version_mismatches().len())
            .await
            .unwrap();
        assert_eq!(after, 0);

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_reentrant_handler_request_connected_succeeds() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let callback_request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "callback",
                "params": { "phase": "reentrant" },
            });
            let mut callback_line = serde_json::to_string(&callback_request).unwrap();
            callback_line.push('\n');
            server_write
                .write_all(callback_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let nested_request_line = lines.next_line().await.unwrap().unwrap();
            let nested_request: Value = serde_json::from_str(&nested_request_line).unwrap();
            assert_eq!(nested_request["method"], "nested");
            let nested_request_id = nested_request["id"].clone();

            let nested_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": nested_request_id,
                "result": { "nested": true },
            });
            let mut nested_response_line = serde_json::to_string(&nested_response).unwrap();
            nested_response_line.push('\n');
            server_write
                .write_all(nested_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let callback_response_line = lines.next_line().await.unwrap().unwrap();
            let callback_response: Value = serde_json::from_str(&callback_response_line).unwrap();
            assert_eq!(callback_response["id"], 99);
            assert_eq!(
                callback_response["result"],
                serde_json::json!({ "nested": true })
            );

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let shared_slot: Arc<StdMutex<Option<SharedManager>>> = Arc::new(StdMutex::new(None));
        let handler_slot = shared_slot.clone();
        let handler: ServerRequestHandler = Arc::new(move |_| {
            let shared = handler_slot
                .lock()
                .unwrap()
                .as_ref()
                .expect("shared manager installed")
                .clone();
            Box::pin(async move {
                let nested = shared
                    .request_connected("srv", "nested", None::<Value>)
                    .await
                    .expect("connected shared request should release manager lock during I/O");
                ServerRequestOutcome::Ok(nested)
            })
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_request_handler(handler);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        *shared_slot.lock().unwrap() = Some(shared.clone());

        let result = shared.request_connected("srv", "ping", None).await.unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_reentrant_handler_request_typed_connected_succeeds() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let callback_request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "callback",
                "params": { "phase": "typed" },
            });
            let mut callback_line = serde_json::to_string(&callback_request).unwrap();
            callback_line.push('\n');
            server_write
                .write_all(callback_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let nested_request_line = lines.next_line().await.unwrap().unwrap();
            let nested_request: Value = serde_json::from_str(&nested_request_line).unwrap();
            assert_eq!(nested_request["method"], "nested");
            assert_eq!(
                nested_request["params"],
                serde_json::json!({ "phase": "typed" })
            );
            let nested_request_id = nested_request["id"].clone();

            let nested_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": nested_request_id,
                "result": { "nested": true },
            });
            let mut nested_response_line = serde_json::to_string(&nested_response).unwrap();
            nested_response_line.push('\n');
            server_write
                .write_all(nested_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let callback_response_line = lines.next_line().await.unwrap().unwrap();
            let callback_response: Value = serde_json::from_str(&callback_response_line).unwrap();
            assert_eq!(callback_response["id"], 99);
            assert_eq!(
                callback_response["result"],
                serde_json::json!({ "nested": true })
            );

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let shared_slot: Arc<StdMutex<Option<SharedManager>>> = Arc::new(StdMutex::new(None));
        let handler_slot = shared_slot.clone();
        let handler: ServerRequestHandler = Arc::new(move |_| {
            let shared = handler_slot
                .lock()
                .unwrap()
                .as_ref()
                .expect("shared manager installed")
                .clone();
            Box::pin(async move {
                let nested = shared
                    .request_typed_connected::<NestedRequest>(
                        "srv",
                        Some(NestedParams { phase: "typed" }),
                    )
                    .await
                    .expect(
                        "typed connected shared request should release manager lock during I/O",
                    );
                ServerRequestOutcome::Ok(serde_json::to_value(nested).unwrap())
            })
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_request_handler(handler);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        *shared_slot.lock().unwrap() = Some(shared.clone());

        let result = shared.request_connected("srv", "ping", None).await.unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_reentrant_handler_is_connected_fails_fast_on_manager_lock() {
        use tokio::sync::oneshot;

        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let (send_callback_tx, send_callback_rx) = oneshot::channel();
        let (callback_done_tx, callback_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            send_callback_rx.await.unwrap();

            let callback_request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "callback",
                "params": { "phase": "lock-held" },
            });
            let mut callback_line = serde_json::to_string(&callback_request).unwrap();
            callback_line.push('\n');
            server_write
                .write_all(callback_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let callback_response_line = lines.next_line().await.unwrap().unwrap();
            let callback_response: Value = serde_json::from_str(&callback_response_line).unwrap();
            assert_eq!(callback_response["id"], 99);
            let error_message = callback_response["error"]["message"]
                .as_str()
                .expect("callback should receive error response");
            assert!(
                error_message.contains(super::REENTRANT_HANDLER_ERROR),
                "unexpected callback error: {callback_response}"
            );
            assert!(
                error_message.contains("is_connected"),
                "callback error should identify the blocked operation: {callback_response}"
            );
            let _ = callback_done_tx.send(());

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after shared disconnect");
        });

        let shared_slot: Arc<StdMutex<Option<SharedManager>>> = Arc::new(StdMutex::new(None));
        let handler_slot = shared_slot.clone();
        let handler: ServerRequestHandler = Arc::new(move |_| {
            let shared = handler_slot
                .lock()
                .unwrap()
                .as_ref()
                .expect("shared manager installed")
                .clone();
            Box::pin(async move {
                match shared.is_connected("srv").await {
                    Ok(connected) => ServerRequestOutcome::Ok(serde_json::json!({
                        "connected": connected,
                    })),
                    Err(err) => ServerRequestOutcome::Error {
                        code: -32001,
                        message: format!("{err:#}"),
                        data: None,
                    },
                }
            })
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_request_handler(handler);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();

        let shared = manager.into_shared();
        *shared_slot.lock().unwrap() = Some(shared.clone());

        let (lock_started_tx, lock_started_rx) = oneshot::channel();
        let (release_lock_tx, release_lock_rx) = std::sync::mpsc::channel();
        let lock_shared = shared.clone();
        let runtime = tokio::runtime::Handle::current();
        let lock_thread = std::thread::spawn(move || {
            runtime.block_on(async move {
                lock_shared
                    .inspect(|_| {
                        let _ = lock_started_tx.send(());
                        release_lock_rx.recv().expect("release manager lock");
                    })
                    .await
                    .expect("inspect should succeed outside handler");
            });
        });

        lock_started_rx.await.unwrap();

        send_callback_tx.send(()).unwrap();
        callback_done_rx.await.unwrap();
        release_lock_tx.send(()).unwrap();
        lock_thread.join().unwrap();
        assert!(shared.disconnect("srv").await.unwrap());
        server_task.await.unwrap();
    }

    #[cfg(unix)]
    fn unix_socket_temp_roots() -> Vec<std::path::PathBuf> {
        let mut roots = Vec::new();

        if let Some(root) = std::env::var_os("OMNE_TEST_SHORT_TMPDIR") {
            let root = std::path::PathBuf::from(root);
            if !roots.iter().any(|candidate| candidate == &root) {
                roots.push(root);
            }
        }

        let temp_dir = std::env::temp_dir();
        if !roots.iter().any(|candidate| candidate == &temp_dir) {
            roots.push(temp_dir);
        }

        if std::env::var_os("TMPDIR").is_none()
            && std::env::temp_dir() == std::path::Path::new("/tmp")
        {
            let root = std::path::PathBuf::from("/var/tmp");
            if !roots.iter().any(|candidate| candidate == &root) {
                roots.push(root);
            }
        }

        roots
    }

    #[cfg(unix)]
    fn unique_socket_path(test_name: &str, label: &str) -> Option<std::path::PathBuf> {
        use std::os::unix::net::UnixListener;
        use std::time::{SystemTime, UNIX_EPOCH};

        let short_label: String = label
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .take(8)
            .collect();

        for root in unix_socket_temp_roots() {
            if !root.exists() && std::fs::create_dir_all(&root).is_err() {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&root) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }

            let Ok(tempdir) = tempfile::Builder::new()
                .prefix("of-sm-")
                .rand_bytes(3)
                .tempdir_in(&root)
            else {
                continue;
            };

            let path = tempdir.path().join(format!(
                "{short_label}-{}-{}.sock",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            if let Ok(listener) = UnixListener::bind(&path) {
                drop(listener);
                let _ = std::fs::remove_file(&path);
                let _persisted_root = tempdir.keep();
                return Some(path);
            }
        }

        eprintln!(
            "skipping {test_name}: unable to create a short writable temp dir for unix socket test"
        );
        None
    }

    #[cfg(unix)]
    fn bind_unix_listener_or_skip(path: &std::path::Path) -> Option<tokio::net::UnixListener> {
        match tokio::net::UnixListener::bind(path) {
            Ok(listener) => Some(listener),
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping shared_manager unix-socket test: unix listener bind not permitted in this environment: {err}"
                );
                None
            }
            Err(err) => panic!(
                "failed to bind shared_manager unix listener at {}: {err}",
                path.display()
            ),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_cold_start_initialize_does_not_block_other_servers() {
        use tokio::sync::oneshot;

        let Some(slow_socket_path) = unique_socket_path(
            "shared_manager_cold_start_initialize_does_not_block_other_servers",
            "slow",
        ) else {
            return;
        };
        let Some(fast_socket_path) = unique_socket_path(
            "shared_manager_cold_start_initialize_does_not_block_other_servers",
            "fast",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&slow_socket_path);
        let _ = std::fs::remove_file(&fast_socket_path);

        let Some(slow_listener) = bind_unix_listener_or_skip(&slow_socket_path) else {
            return;
        };
        let Some(fast_listener) = bind_unix_listener_or_skip(&fast_socket_path) else {
            return;
        };
        let (slow_init_seen_tx, slow_init_seen_rx) = oneshot::channel();
        let (release_slow_tx, release_slow_rx) = oneshot::channel();

        let slow_server_task = tokio::spawn(async move {
            let (stream, _) = slow_listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();
            let _ = slow_init_seen_tx.send(());

            release_slow_rx.await.unwrap();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "slow" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "server": "slow" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after shared disconnect");
        });

        let fast_server_task = tokio::spawn(async move {
            let (stream, _) = fast_listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "fast" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "server": "fast" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after shared disconnect");
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("slow").unwrap(),
            ServerConfig::unix(slow_socket_path.clone()).unwrap(),
        );
        servers.insert(
            ServerName::parse("fast").unwrap(),
            ServerConfig::unix(fast_socket_path.clone()).unwrap(),
        );
        let config = Arc::new(Config::new(ClientConfig::default(), servers));

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();

        let slow_shared = shared.clone();
        let slow_config = Arc::clone(&config);
        let slow_request = tokio::spawn(async move {
            slow_shared
                .request(
                    slow_config.as_ref(),
                    "slow",
                    "ping",
                    None::<Value>,
                    Path::new("/"),
                )
                .await
        });

        slow_init_seen_rx.await.unwrap();

        let fast_result = tokio::time::timeout(Duration::from_millis(250), async {
            shared
                .request(
                    config.as_ref(),
                    "fast",
                    "ping",
                    None::<Value>,
                    Path::new("/"),
                )
                .await
        })
        .await
        .expect("fast cold-start should not be blocked by slow initialize")
        .unwrap();
        assert_eq!(fast_result, serde_json::json!({ "server": "fast" }));

        release_slow_tx.send(()).unwrap();

        let slow_result = tokio::time::timeout(Duration::from_secs(1), slow_request)
            .await
            .expect("slow request should finish after initialize is released")
            .unwrap()
            .unwrap();
        assert_eq!(slow_result, serde_json::json!({ "server": "slow" }));

        assert!(shared.disconnect("slow").await.unwrap());
        assert!(shared.disconnect("fast").await.unwrap());
        slow_server_task.await.unwrap();
        fast_server_task.await.unwrap();

        let _ = std::fs::remove_file(slow_socket_path);
        let _ = std::fs::remove_file(fast_socket_path);
    }

    #[tokio::test]
    async fn shared_manager_notify_can_overlap_with_in_flight_request_on_same_server() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let (request_seen_tx, request_seen_rx) = oneshot::channel();
        let (notify_seen_tx, notify_seen_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();
            request_seen_tx.send(()).unwrap();

            let notification_line =
                tokio::time::timeout(Duration::from_millis(200), lines.next_line())
                    .await
                    .expect("same-server notify should overlap an in-flight request")
                    .unwrap()
                    .unwrap();
            let notification_value: Value = serde_json::from_str(&notification_line).unwrap();
            assert_eq!(notification_value["method"], "demo/notify");
            notify_seen_tx.send(()).unwrap();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let cwd = test_workspace_path("overlap");
        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::stdio(vec!["mock-server".to_string()]).unwrap(),
        );
        let config = Arc::new(Config::new(ClientConfig::default(), servers));

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();
        manager.record_connection_cwd("srv", &cwd).unwrap();
        manager
            .record_connection_server_config("srv", config.server("srv").unwrap())
            .unwrap();

        let shared = manager.into_shared();
        let request_shared = shared.clone();
        let request_config = Arc::clone(&config);
        let request_cwd = cwd.clone();
        let request_task = tokio::spawn(async move {
            request_shared
                .request(
                    request_config.as_ref(),
                    "srv",
                    "ping",
                    None::<Value>,
                    &request_cwd,
                )
                .await
        });

        request_seen_rx.await.unwrap();
        tokio::time::timeout(
            Duration::from_millis(200),
            shared.notify_connected("srv", "demo/notify", None::<Value>),
        )
        .await
        .expect("notify should not wait for the request response")
        .unwrap();
        notify_seen_rx.await.unwrap();

        let result = request_task.await.unwrap().unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));
        server_task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_reentrant_handler_cold_start_fails_fast_on_connect_gate() {
        let Some(socket_path) = unique_socket_path(
            "shared_manager_reentrant_handler_cold_start_fails_fast_on_connect_gate",
            "reentrant-cold-start",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let callback_request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "callback",
                "params": { "phase": "cold-start" },
            });
            let mut callback_line = serde_json::to_string(&callback_request).unwrap();
            callback_line.push('\n');
            server_write
                .write_all(callback_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let callback_response_line = lines.next_line().await.unwrap().unwrap();
            let callback_response: Value = serde_json::from_str(&callback_response_line).unwrap();
            assert_eq!(callback_response["id"], 99);
            let error_message = callback_response["error"]["message"]
                .as_str()
                .expect("callback should receive error response");
            assert!(
                error_message.contains(super::REENTRANT_HANDLER_ERROR),
                "unexpected callback error: {callback_response}"
            );

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after shared disconnect");
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Arc::new(Config::new(ClientConfig::default(), servers));
        let shared_slot: Arc<StdMutex<Option<SharedManager>>> = Arc::new(StdMutex::new(None));

        let handler_slot = Arc::clone(&shared_slot);
        let handler_config = Arc::clone(&config);
        let handler: ServerRequestHandler = Arc::new(move |_| {
            let shared = handler_slot
                .lock()
                .unwrap()
                .as_ref()
                .expect("shared manager installed")
                .clone();
            let config = Arc::clone(&handler_config);
            Box::pin(async move {
                match shared
                    .request(
                        config.as_ref(),
                        "srv",
                        "nested",
                        None::<Value>,
                        Path::new("/"),
                    )
                    .await
                {
                    Ok(result) => ServerRequestOutcome::Ok(result),
                    Err(err) => ServerRequestOutcome::Error {
                        code: -32001,
                        message: format!("{err:#}"),
                        data: None,
                    },
                }
            })
        });

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .with_server_request_handler(handler)
            .into_shared();
        *shared_slot.lock().unwrap() = Some(shared.clone());

        let result = tokio::time::timeout(Duration::from_secs(1), async {
            shared
                .request(
                    config.as_ref(),
                    "srv",
                    "ping",
                    None::<Value>,
                    Path::new("/"),
                )
                .await
        })
        .await
        .expect("outer cold-start request should not deadlock")
        .unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        assert!(shared.disconnect("srv").await.unwrap());
        server_task.await.unwrap();

        let _ = std::fs::remove_file(socket_path);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shared_manager_request_resolves_relative_cwd_from_config_thread_root() {
        let _guard = cwd_test_guard_async().await;
        let _cwd_restore = CurrentDirRestoreGuard::capture();
        let tempdir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let config_dir = tempdir.path().join("configs");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::env::set_current_dir(tempdir.path()).expect("enter config dir parent");

        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();
            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(PathBuf::from("/tmp/mock.sock")).unwrap(),
        );
        let config = Config::new(ClientConfig::default(), servers)
            .with_path(PathBuf::from("configs/mcp.json"));
        std::env::set_current_dir(outside.path()).expect("enter outside dir");

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .expect("connect in-memory client");
        manager
            .record_connection_cwd_with_base(
                "srv",
                Path::new("workspace/demo"),
                config.thread_root(),
            )
            .expect("record cwd identity");
        manager
            .record_connection_server_config_effective_with_base(
                "srv",
                config.server("srv").expect("config server"),
                Path::new("workspace/demo"),
                config.thread_root(),
                config.thread_root(),
            )
            .expect("record config identity");

        let shared = manager.into_shared();
        let result = shared
            .request(
                &config,
                "srv",
                "ping",
                None::<Value>,
                Path::new("workspace/./demo"),
            )
            .await
            .expect("same thread-root-relative cwd should reuse connection");
        assert_eq!(result, serde_json::json!({ "ok": true }));
        server_task.await.unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shared_manager_request_rejects_different_effective_config() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();
            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "request should fail before reuse I/O");
        });

        let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted);
        manager
            .connect_io("srv", client_read, client_write)
            .await
            .unwrap();
        let connected_cwd = test_workspace_path("a");
        manager
            .record_connection_cwd("srv", &connected_cwd)
            .unwrap();
        manager
            .record_connection_server_config(
                "srv",
                &ServerConfig::unix(PathBuf::from("/tmp/original.sock")).unwrap(),
            )
            .unwrap();

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(PathBuf::from("/tmp/changed.sock")).unwrap(),
        );
        let config = Config::new(ClientConfig::default(), servers);

        let shared = manager.into_shared();
        let err = shared
            .request(&config, "srv", "ping", None::<Value>, &connected_cwd)
            .await
            .expect_err("different effective config should not be silently reused");
        assert!(
            err.to_string().contains("different effective config"),
            "{err:#}"
        );
        drop(shared);
        server_task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_request_rejects_different_cwd_on_reuse() {
        let Some(socket_path) = unique_socket_path(
            "shared_manager_request_rejects_different_cwd_on_reuse",
            "cwd-reuse",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };
        let (idle_tx, idle_rx) = tokio::sync::oneshot::channel();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "ok": true },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            assert!(
                tokio::time::timeout(Duration::from_millis(100), lines.next_line())
                    .await
                    .is_err(),
                "different cwd reuse should fail before a second request is sent"
            );
            idle_tx
                .send(())
                .expect("idle window signal should be delivered");

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after shared disconnect");
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Config::new(ClientConfig::default(), servers);
        let first_cwd = test_workspace_path("a");
        let second_cwd = test_workspace_path("b");

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();

        let first = shared
            .request(&config, "srv", "ping", None::<Value>, &first_cwd)
            .await
            .unwrap();
        assert_eq!(first, serde_json::json!({ "ok": true }));

        let err = shared
            .request(&config, "srv", "ping", None::<Value>, &second_cwd)
            .await
            .expect_err("different cwd should be rejected");
        assert!(
            err.to_string().contains("cannot be reused for cwd="),
            "{err:#}"
        );
        assert!(shared.is_connected("srv").await.unwrap());
        idle_rx.await.unwrap();
        assert!(shared.disconnect("srv").await.unwrap());

        server_task.await.unwrap();
        let _ = std::fs::remove_file(socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_concurrent_cold_start_requests_share_single_connection() {
        let Some(socket_path) = unique_socket_path(
            "shared_manager_concurrent_cold_start_requests_share_single_connection",
            "single-flight",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            for _ in 0..2 {
                let request_line = lines.next_line().await.unwrap().unwrap();
                let request_value: Value = serde_json::from_str(&request_line).unwrap();
                assert_eq!(request_value["method"], "ping");
                let request_id = request_value["id"].clone();

                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": { "ok": true },
                });
                let mut response_line = serde_json::to_string(&response).unwrap();
                response_line.push('\n');
                server_write
                    .write_all(response_line.as_bytes())
                    .await
                    .unwrap();
                server_write.flush().await.unwrap();
            }

            let eof = lines.next_line().await.unwrap();
            assert!(eof.is_none(), "expected EOF after shared disconnect");

            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err(),
                "expected no second cold-start connection"
            );
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Config::new(ClientConfig::default(), servers);

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();

        let clone = shared.clone();
        let (result_a, result_b) = tokio::join!(
            shared.request(&config, "srv", "ping", None, Path::new("/")),
            clone.request(&config, "srv", "ping", None, Path::new("/")),
        );
        assert_eq!(result_a.unwrap(), serde_json::json!({ "ok": true }));
        assert_eq!(result_b.unwrap(), serde_json::json!({ "ok": true }));

        assert!(shared.disconnect("srv").await.unwrap());
        server_task.await.unwrap();

        let _ = std::fs::remove_file(socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_disconnect_waits_until_borrowed_client_gate_is_released() {
        let Some(socket_path) = unique_socket_path(
            "shared_manager_disconnect_waits_until_borrowed_client_gate_is_released",
            "borrowed-client-gate",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let (first_init_seen_tx, first_init_seen_rx) = oneshot::channel();
        let (release_first_init_tx, release_first_init_rx) = oneshot::channel();
        let (connection_closed_tx, connection_closed_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();
            first_init_seen_tx.send(()).unwrap();
            release_first_init_rx.await.unwrap();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo-a" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let eof = lines.next_line().await.unwrap();
            connection_closed_tx.send(()).unwrap();
            assert!(eof.is_none(), "connection should close after disconnect");
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Arc::new(Config::new(ClientConfig::default(), servers));

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();
        let server_name = ServerName::parse("srv").unwrap();

        let (prepared_ready_tx, prepared_ready_rx) = oneshot::channel();
        let (release_io_tx, release_io_rx) = oneshot::channel();
        let first_shared = shared.clone();
        let first_config = Arc::clone(&config);
        let prepared_client = tokio::spawn(async move {
            let connected_cwd = test_workspace_path("a");
            let prepared = first_shared
                .prepare_connected_client_with_gate(
                    "test_prepare",
                    first_config.as_ref(),
                    &server_name,
                    &connected_cwd,
                )
                .await
                .unwrap();
            let in_flight_io = prepared
                .in_flight_io
                .expect("prepared client should retain an in-flight IO guard");
            prepared_ready_tx.send(()).unwrap();
            release_io_rx.await.unwrap();
            drop(in_flight_io);
        });

        first_init_seen_rx.await.unwrap();

        let disconnect_finished = Arc::new(AtomicBool::new(false));
        let disconnect_shared = shared.clone();
        let disconnect_finished_task = Arc::clone(&disconnect_finished);
        let disconnect_task = tokio::spawn(async move {
            let disconnected = disconnect_shared.disconnect("srv").await.unwrap();
            assert!(
                disconnected,
                "disconnect should tear down the borrowed connection"
            );
            disconnect_finished_task.store(true, Ordering::SeqCst);
        });

        release_first_init_tx.send(()).unwrap();

        prepared_ready_rx
            .await
            .expect("first cold-start should borrow client before disconnect proceeds");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !disconnect_finished.load(Ordering::SeqCst),
            "disconnect should still be blocked while the borrowed client IO guard is held"
        );

        release_io_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            disconnect_task.await.unwrap();
            connection_closed_rx.await.unwrap()
        })
        .await
        .expect("disconnect should finish once the borrowed client IO guard is released");

        tokio::time::timeout(Duration::from_secs(1), prepared_client)
            .await
            .unwrap()
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server task should observe the disconnect sequence")
            .unwrap();
        let _ = std::fs::remove_file(socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_manager_cold_start_request_holds_gate_until_first_io_completes() {
        let Some(socket_path) = unique_socket_path(
            "shared_manager_cold_start_request_holds_gate_until_first_io_completes",
            "request-first-io-gate",
        ) else {
            return;
        };
        let _ = std::fs::remove_file(&socket_path);
        let Some(listener) = bind_unix_listener_or_skip(&socket_path) else {
            return;
        };

        let (first_init_seen_tx, first_init_seen_rx) = oneshot::channel();
        let (release_first_init_tx, release_first_init_rx) = oneshot::channel();
        let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
        let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
        let (connection_closed_tx, connection_closed_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (server_read, mut server_write) = tokio::io::split(stream);
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let init_id = init_value["id"].clone();
            first_init_seen_tx.send(()).unwrap();
            release_first_init_rx.await.unwrap();

            let init_response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION, "serverInfo": { "name": "demo-a" } },
            });
            let mut init_response_line = serde_json::to_string(&init_response).unwrap();
            init_response_line.push('\n');
            server_write
                .write_all(init_response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let initialized_line = lines.next_line().await.unwrap().unwrap();
            let initialized_value: Value = serde_json::from_str(&initialized_line).unwrap();
            assert_eq!(initialized_value["method"], "notifications/initialized");

            let request_line = lines.next_line().await.unwrap().unwrap();
            let request_value: Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request_value["method"], "ping");
            let request_id = request_value["id"].clone();
            first_request_seen_tx.send(()).unwrap();
            release_first_response_rx.await.unwrap();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "server": "a" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let eof = lines.next_line().await.unwrap();
            connection_closed_tx.send(()).unwrap();
            assert!(
                eof.is_none(),
                "disconnect should land after the first request finishes"
            );
        });

        let mut servers = BTreeMap::new();
        servers.insert(
            ServerName::parse("srv").unwrap(),
            ServerConfig::unix(socket_path.clone()).unwrap(),
        );
        let config = Arc::new(Config::new(ClientConfig::default(), servers));

        let shared = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
            .with_trust_mode(TrustMode::Trusted)
            .into_shared();

        let request_finished = Arc::new(AtomicBool::new(false));
        let request_finished_task = Arc::clone(&request_finished);
        let first_shared = shared.clone();
        let first_config = Arc::clone(&config);
        let first_request = tokio::spawn(async move {
            let connected_cwd = test_workspace_path("a");
            let result = first_shared
                .request(
                    first_config.as_ref(),
                    "srv",
                    "ping",
                    None::<Value>,
                    &connected_cwd,
                )
                .await;
            request_finished_task.store(true, Ordering::SeqCst);
            result
        });

        first_init_seen_rx.await.unwrap();

        let disconnect_finished = Arc::new(AtomicBool::new(false));
        let disconnect_shared = shared.clone();
        let disconnect_finished_task = Arc::clone(&disconnect_finished);
        let disconnect_task = tokio::spawn(async move {
            let disconnected = disconnect_shared.disconnect("srv").await.unwrap();
            assert!(
                disconnected,
                "disconnect should tear down the connection after the first request"
            );
            disconnect_finished_task.store(true, Ordering::SeqCst);
        });

        release_first_init_tx.send(()).unwrap();
        first_request_seen_rx.await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !disconnect_finished.load(Ordering::SeqCst),
            "disconnect should stay blocked while the cold-start request is still in flight"
        );
        assert!(
            !request_finished.load(Ordering::SeqCst),
            "request should still be waiting on the delayed server response"
        );

        release_first_response_tx.send(()).unwrap();

        let first_result = tokio::time::timeout(Duration::from_secs(1), first_request)
            .await
            .expect("first request should finish once the server responds")
            .unwrap()
            .unwrap();
        assert_eq!(first_result, serde_json::json!({ "server": "a" }));

        tokio::time::timeout(Duration::from_secs(1), async {
            disconnect_task.await.unwrap();
            connection_closed_rx.await.unwrap()
        })
        .await
        .expect("disconnect should finish after the first request releases the gate");

        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server task should observe the delayed disconnect")
            .unwrap();
        let _ = std::fs::remove_file(socket_path);
    }
}
