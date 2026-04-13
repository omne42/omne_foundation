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

    async fn ensure_connected_with_write_gate(
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

        self.ensure_connected_with_write_gate(operation, config, server_name, &cwd)
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
                .as_ref()
                .filter(|scope| scope.upgrade().is_some())
                .cloned()
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
mod tests;
