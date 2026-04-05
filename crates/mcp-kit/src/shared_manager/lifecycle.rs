use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::sync::OwnedRwLockReadGuard;

use super::SharedManager;
use crate::{Config, Manager};

pub(super) struct PreparedSharedClient {
    pub(super) prepared: crate::manager::PreparedConnectedClient,
    pub(super) same_server_gate: Option<OwnedRwLockReadGuard<()>>,
}

impl SharedManager {
    pub(super) async fn try_prepare_connected_client(
        &self,
        operation: &'static str,
        server_name: &str,
        cwd: Option<&Path>,
    ) -> anyhow::Result<Option<crate::manager::PreparedConnectedClient>> {
        let resolved_cwd = match cwd {
            Some(cwd) => Some(self.resolve_connection_cwd(operation, None, cwd).await?),
            None => None,
        };
        self.lock_for_async_op(operation)
            .await?
            .try_prepare_connected_client_resolved(server_name, resolved_cwd.as_deref())
    }

    pub(super) async fn try_prepare_reusable_connected_client(
        &self,
        operation: &'static str,
        config: &Config,
        server_name: &str,
        cwd: Option<&Path>,
    ) -> anyhow::Result<Option<crate::manager::PreparedConnectedClient>> {
        let server_cfg = config.server(server_name).ok_or_else(|| {
            crate::error::tagged_message(
                crate::error::ErrorKind::Config,
                format!("unknown mcp server: {server_name}"),
            )
        })?;
        let resolved_cwd = match cwd {
            Some(cwd) => Some(
                self.resolve_connection_cwd(operation, config.thread_root(), cwd)
                    .await?,
            ),
            None => None,
        };
        self.lock_for_async_op(operation)
            .await?
            .try_prepare_reusable_connected_client_resolved(
                server_name,
                server_cfg,
                resolved_cwd.as_deref(),
            )
    }

    pub(super) async fn ensure_connected_while_gated(
        &self,
        operation: &'static str,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> anyhow::Result<()> {
        let cwd = self
            .resolve_connection_cwd(operation, config.thread_root(), cwd)
            .await?;

        if self
            .try_prepare_reusable_connected_client(operation, config, server_name, Some(&cwd))
            .await?
            .is_some()
        {
            return Ok(());
        }

        let prepared = {
            self.lock_for_async_op(operation)
                .await?
                .prepare_transport_connect_resolved(config, server_name, cwd.clone())?
        };
        let Some(prepared) = prepared else {
            return Ok(());
        };

        let (client, child) = Manager::connect_prepared_transport(&prepared).await?;

        let install = self
            .lock_for_async_op(operation)
            .await?
            .prepare_transport_install(
                &prepared.server_name_key,
                &prepared.cwd,
                &prepared.server_cfg,
            );

        let result = install.run(client, child).await;
        self.lock_for_async_op(operation)
            .await?
            .finish_transport_install_attempt(&prepared.server_name_key, result)
    }

    pub(super) async fn prepare_connected_client_with_gate(
        &self,
        operation: &'static str,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> anyhow::Result<PreparedSharedClient> {
        let cwd = self
            .resolve_connection_cwd(operation, config.thread_root(), cwd)
            .await?;

        let read_gate = self.lock_connect_gate_read(operation, server_name).await?;
        if let Some(prepared) = self
            .try_prepare_reusable_connected_client(operation, config, server_name, Some(&cwd))
            .await?
        {
            return Ok(PreparedSharedClient {
                prepared,
                same_server_gate: Some(read_gate),
            });
        }
        drop(read_gate);

        let write_gate = self.lock_connect_gate_write(operation, server_name).await?;

        if let Some(prepared) = self
            .try_prepare_reusable_connected_client(operation, config, server_name, Some(&cwd))
            .await?
        {
            drop(write_gate);
            return Ok(PreparedSharedClient {
                prepared,
                same_server_gate: None,
            });
        }

        self.ensure_connected_while_gated(operation, config, server_name, &cwd)
            .await?;

        let prepared = self
            .try_prepare_connected_client(operation, server_name, Some(&cwd))
            .await?
            .ok_or_else(|| {
                crate::error::tagged_message(
                    crate::error::ErrorKind::ManagerState,
                    format!(
                        "mcp server became unavailable before {operation}: {}",
                        server_name.trim()
                    ),
                )
            })?;
        let same_server_gate = tokio::sync::OwnedRwLockWriteGuard::downgrade(write_gate);
        Ok(PreparedSharedClient {
            prepared,
            same_server_gate: Some(same_server_gate),
        })
    }

    pub(super) async fn prepare_existing_connected_client_with_gate(
        &self,
        operation: &'static str,
        server_name: &str,
    ) -> anyhow::Result<PreparedSharedClient> {
        let gate = self.lock_connect_gate_read(operation, server_name).await?;
        let prepared = self
            .try_prepare_connected_client(operation, server_name, None)
            .await?
            .ok_or_else(|| {
                crate::error::tagged_message(
                    crate::error::ErrorKind::ManagerState,
                    format!("mcp server not connected: {}", server_name.trim()),
                )
            })?;
        Ok(PreparedSharedClient {
            prepared,
            same_server_gate: Some(gate),
        })
    }

    pub(super) async fn resolve_connection_cwd(
        &self,
        operation: &'static str,
        base: Option<&Path>,
        cwd: &Path,
    ) -> anyhow::Result<PathBuf> {
        crate::manager::resolve_connection_cwd_with_base_async(base, cwd)
            .await
            .with_context(|| format!("resolve connection cwd for {operation}"))
    }

    pub(super) async fn cleanup_connection_after_error(
        &self,
        server_name: String,
        connection_id: u64,
    ) {
        let disconnect = if self.is_reentrant_handler_call() {
            match self.inner.try_lock() {
                Ok(mut manager) => manager
                    .prepare_disconnect_for_wait_if_connection_with_cwd_cleanup(
                        &server_name,
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
                &server_name,
                connection_id,
            )
        };
        disconnect.wait_for_jsonrpc_error_cleanup().await;
    }

    pub(super) fn spawn_connection_cleanup(&self, server_name: String, connection_id: u64) {
        let shared = self.clone();
        tokio::spawn(async move {
            let disconnect = {
                let mut manager = shared.inner.lock().await;
                manager.prepare_disconnect_for_wait_if_connection_with_cwd_cleanup(
                    &server_name,
                    connection_id,
                )
            };
            disconnect.wait_for_jsonrpc_error_cleanup().await;
        });
    }
}
