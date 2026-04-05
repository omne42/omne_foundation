mod lifecycle;
mod rpc;

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::{Manager, ProtocolVersionMismatch, ServerName};

const REENTRANT_HANDLER_ERROR: &str = "SharedManager async operations cannot be called reentrantly from the wrapped manager's server handlers while another operation is in flight";

/// Cloneable wrapper around `Manager` for serialized shared async use.
///
/// This wrapper is intentionally a single-flight lifecycle gate, not an actor. It serializes
/// manager state access through a single async mutex, while same-server connect/disconnect paths
/// also share a per-server gate so cold starts and teardown cannot overlap. Config-driven
/// request/notify operations downgrade the lifecycle gate to a same-server read lock before
/// awaiting JSON-RPC I/O, including the cold-start path after the freshly installed connection
/// has been prepared for use. That read gate stays alive until the in-flight request/notify
/// finishes, so concurrent same-server `disconnect` cannot tear down the borrowed connection
/// underneath it while sibling request/notify operations can still overlap. Lifecycle and
/// inspection operations still execute under the shared lock.
///
/// Reentrant fail-fast applies when a handler-scoped task calls back into `SharedManager` and the
/// operation would otherwise have to wait on the shared lock or a same-server gate. Bare
/// `tokio::spawn(...)` does not inherit that handler scope automatically, so external callers and
/// non-inherited child tasks keep the normal waiting behavior unless they opt into
/// [`SharedManager::spawn_inheriting_handler_scope`].
///
/// This is not an actor or fully concurrent manager:
/// - lifecycle-changing operations still serialize on the shared manager lock
/// - connected request/notify operations can overlap with each other while still blocking
///   same-server disconnect until their I/O finishes
/// - operations that still need the shared lock or same-server gate return an error when an
///   inherited handler-scoped path would otherwise wait, instead of risking deadlock
/// - connect/disconnect lifecycle changes for the same server share a single gate, and
///   `disconnect_and_wait` keeps that gate until its wait finishes so a slow teardown cannot race
///   with a replacement cold start
#[derive(Clone)]
pub struct SharedManager {
    inner: Arc<Mutex<Manager>>,
    connect_gates: Arc<StdMutex<HashMap<String, Weak<RwLock<()>>>>>,
    manager_id: u64,
    active_handler_scopes: Arc<AtomicU64>,
}

impl Manager {
    /// Converts this manager into a cloneable single-flight wrapper.
    pub fn into_shared(self) -> SharedManager {
        SharedManager::new(self)
    }
}

impl SharedManager {
    pub fn new(manager: Manager) -> Self {
        let manager_id = manager.instance_id();
        let active_handler_scopes = crate::manager::Manager::active_handler_scopes(&manager);
        Self {
            inner: Arc::new(Mutex::new(manager)),
            connect_gates: Arc::new(StdMutex::new(HashMap::new())),
            manager_id,
            active_handler_scopes,
        }
    }

    pub fn try_unwrap(self) -> Result<Manager, Self> {
        match Arc::try_unwrap(self.inner) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(inner) => Err(Self {
                inner,
                connect_gates: self.connect_gates,
                manager_id: self.manager_id,
                active_handler_scopes: self.active_handler_scopes,
            }),
        }
    }

    fn is_reentrant_handler_call(&self) -> bool {
        crate::manager::is_in_manager_handler_scope(self.manager_id)
    }

    /// Spawn a task that preserves the current manager handler scope.
    ///
    /// Tokio `spawn` does not inherit `task_local!` state automatically, so bare child tasks
    /// behave like external callers and wait normally. This helper preserves handler-scope
    /// fail-fast semantics even if the child outlives the parent task or runs after the parent
    /// handler returns.
    pub fn spawn_inheriting_handler_scope<F>(&self, fut: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let manager_id = self.manager_id;
        let active_handler_scopes = Arc::clone(&self.active_handler_scopes);
        let inherit_scope = self.is_reentrant_handler_call();

        tokio::spawn(async move {
            if inherit_scope {
                crate::manager::scope_manager_handler_call(manager_id, active_handler_scopes, fut)
                    .await
            } else {
                fut.await
            }
        })
    }

    fn fail_fast_if_reentrant<T>(
        &self,
        operation: &'static str,
        try_acquire: impl FnOnce() -> Result<T, tokio::sync::TryLockError>,
    ) -> anyhow::Result<Option<T>> {
        // Fail fast only for handler-scoped tasks that would otherwise wait behind their own
        // in-flight shared-manager lock/gate acquisition. External callers, including bare
        // `tokio::spawn(...)` children, keep the normal waiting behavior.
        if !self.is_reentrant_handler_call() {
            return Ok(None);
        }

        try_acquire().map(Some).map_err(|_| {
            crate::error::tagged_message(
                crate::error::ErrorKind::ManagerState,
                format!("{REENTRANT_HANDLER_ERROR}: {operation}"),
            )
        })
    }

    fn connect_gate_for(&self, server_name: &str) -> Arc<RwLock<()>> {
        let key = server_name.trim().to_string();
        let mut gates = self
            .connect_gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(existing) = gates.get(&key).and_then(Weak::upgrade) {
            return existing;
        }

        let gate = Arc::new(RwLock::new(()));
        gates.insert(key, Arc::downgrade(&gate));
        gate
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
        server_name: &str,
    ) -> anyhow::Result<OwnedRwLockWriteGuard<()>> {
        let gate = self.connect_gate_for(server_name);
        if let Some(guard) =
            self.fail_fast_if_reentrant(operation, || gate.clone().try_write_owned())?
        {
            return Ok(guard);
        }
        Ok(gate.write_owned().await)
    }

    async fn lock_connect_gate_read(
        &self,
        operation: &'static str,
        server_name: &str,
    ) -> anyhow::Result<OwnedRwLockReadGuard<()>> {
        let gate = self.connect_gate_for(server_name);
        if let Some(guard) =
            self.fail_fast_if_reentrant(operation, || gate.clone().try_read_owned())?
        {
            return Ok(guard);
        }
        Ok(gate.read_owned().await)
    }

    /// Inspect manager state under the shared lock without exposing borrowed data directly.
    pub async fn inspect<R>(&self, f: impl FnOnce(&Manager) -> R) -> crate::Result<R> {
        Ok(self
            .with_manager_lock("inspect", |manager| f(manager))
            .await?)
    }

    /// Returns whether the named server is still connected.
    ///
    /// This refreshes liveness under the shared manager lock and prunes dead
    /// cached connections before returning `false`.
    pub async fn is_connected(&self, server_name: &str) -> crate::Result<bool> {
        Ok(self
            .with_manager_lock("is_connected", |manager| manager.is_connected(server_name))
            .await?)
    }

    /// Returns whether a connection entry is currently cached for `server_name`
    /// without probing liveness or pruning dead state.
    pub async fn is_connected_cached(&self, server_name: &str) -> crate::Result<bool> {
        self.inspect(|manager| manager.is_connected_cached(server_name))
            .await
    }

    pub async fn is_connected_named(&self, server_name: &ServerName) -> crate::Result<bool> {
        self.is_connected(server_name.as_str()).await
    }

    pub async fn is_connected_cached_named(&self, server_name: &ServerName) -> crate::Result<bool> {
        self.is_connected_cached(server_name.as_str()).await
    }

    /// Returns the names of currently connected servers.
    ///
    /// Like [`SharedManager::is_connected`], this refreshes liveness and prunes
    /// dead cached connections before returning the result.
    pub async fn connected_server_names(&self) -> crate::Result<Vec<ServerName>> {
        Ok(self
            .with_manager_lock("connected_server_names", |manager| {
                manager.connected_server_names()
            })
            .await?)
    }

    /// Returns the cached connection names without probing liveness or pruning
    /// dead state.
    pub async fn connected_server_names_cached(&self) -> crate::Result<Vec<ServerName>> {
        self.inspect(Manager::connected_server_names_cached).await
    }

    pub async fn disconnect(&self, server_name: &str) -> crate::Result<bool> {
        let _gate = self
            .lock_connect_gate_write("disconnect", server_name)
            .await?;
        Ok(self
            .with_manager_lock("disconnect", |manager| manager.disconnect(server_name))
            .await?)
    }

    pub async fn disconnect_named(&self, server_name: &ServerName) -> crate::Result<bool> {
        self.disconnect(server_name.as_str()).await
    }

    pub async fn disconnect_and_wait(
        &self,
        server_name: &str,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        let _gate = self
            .lock_connect_gate_write("disconnect_and_wait", server_name)
            .await?;
        let disconnect = self
            .lock_for_async_op("disconnect_and_wait")
            .await?
            .prepare_disconnect_for_wait_with_cwd_cleanup(server_name);
        Ok(disconnect.wait_with_timeout(timeout, on_timeout).await?)
    }

    pub async fn disconnect_and_wait_named(
        &self,
        server_name: &ServerName,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        self.disconnect_and_wait(server_name.as_str(), timeout, on_timeout)
            .await
    }

    pub async fn server_handler_timeout_count(&self, server_name: &str) -> crate::Result<u64> {
        Ok(self
            .with_manager_lock("server_handler_timeout_count", |manager| {
                manager.server_handler_timeout_count(server_name)
            })
            .await?)
    }

    pub async fn server_handler_timeout_count_named(
        &self,
        server_name: &ServerName,
    ) -> crate::Result<u64> {
        self.server_handler_timeout_count(server_name.as_str())
            .await
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

#[cfg(test)]
mod tests;
