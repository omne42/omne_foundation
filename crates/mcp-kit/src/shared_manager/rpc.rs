use std::path::Path;

use serde_json::Value;

use super::{SharedManager, lifecycle::PreparedSharedClient};
use crate::{Config, Manager, McpNotification, McpRequest, ServerName};
impl SharedManager {
    async fn request_with_prepared_client(
        &self,
        prepared: PreparedSharedClient,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        let PreparedSharedClient {
            prepared,
            same_server_gate: _same_server_gate,
        } = prepared;
        let result = Manager::request_raw_handle(
            prepared.timeout,
            &prepared.server_name,
            &prepared.client,
            method,
            params,
        )
        .await;
        if let Err(err) = &result {
            if crate::manager::should_disconnect_after_jsonrpc_error(err) {
                self.cleanup_connection_after_error(
                    prepared.server_name.clone(),
                    prepared.connection_id,
                )
                .await;
            }
        }
        Ok(result?)
    }

    async fn notify_with_prepared_client(
        &self,
        prepared: PreparedSharedClient,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        let PreparedSharedClient {
            prepared,
            same_server_gate: _same_server_gate,
        } = prepared;
        let result = Manager::notify_raw_handle(
            prepared.timeout,
            &prepared.server_name,
            &prepared.client,
            method,
            params,
        )
        .await;
        if let Err(err) = &result {
            if crate::manager::should_disconnect_after_jsonrpc_error(err)
                || crate::manager::contains_wait_timeout(err)
            {
                self.cleanup_connection_after_error(
                    prepared.server_name.clone(),
                    prepared.connection_id,
                )
                .await;
            }
        }
        Ok(result?)
    }

    pub async fn request(
        &self,
        config: &Config,
        server_name: &str,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let prepared = self
            .prepare_connected_client_with_gate("request", config, server_name, cwd)
            .await?;
        self.request_with_prepared_client(prepared, method, params)
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
        self.request(config, server_name.as_str(), method, params, cwd)
            .await
    }

    pub async fn request_connected(
        &self,
        server_name: &str,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        let prepared = self
            .prepare_existing_connected_client_with_gate("request_connected", server_name)
            .await?;
        self.request_with_prepared_client(prepared, method, params)
            .await
    }

    pub async fn request_connected_named(
        &self,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<Value> {
        self.request_connected(server_name.as_str(), method, params)
            .await
    }

    pub async fn request_typed<R: McpRequest>(
        &self,
        config: &Config,
        server_name: &str,
        params: Option<R::Params>,
        cwd: &Path,
    ) -> crate::Result<R::Result> {
        let params = crate::mcp::serialize_request_params::<R>(server_name, params)?;
        let result = self
            .request(config, server_name, R::METHOD, params, cwd)
            .await?;
        crate::mcp::deserialize_request_result::<R>(server_name, result)
    }

    pub async fn request_typed_named<R: McpRequest>(
        &self,
        config: &Config,
        server_name: &ServerName,
        params: Option<R::Params>,
        cwd: &Path,
    ) -> crate::Result<R::Result> {
        self.request_typed::<R>(config, server_name.as_str(), params, cwd)
            .await
    }

    pub async fn request_typed_connected<R: McpRequest>(
        &self,
        server_name: &str,
        params: Option<R::Params>,
    ) -> crate::Result<R::Result> {
        let params = crate::mcp::serialize_request_params::<R>(server_name, params)?;
        let result = self
            .request_connected(server_name, R::METHOD, params)
            .await?;
        crate::mcp::deserialize_request_result::<R>(server_name, result)
    }

    pub async fn request_typed_connected_named<R: McpRequest>(
        &self,
        server_name: &ServerName,
        params: Option<R::Params>,
    ) -> crate::Result<R::Result> {
        self.request_typed_connected::<R>(server_name.as_str(), params)
            .await
    }

    pub async fn notify(
        &self,
        config: &Config,
        server_name: &str,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<()> {
        let prepared = self
            .prepare_connected_client_with_gate("notify", config, server_name, cwd)
            .await?;
        self.notify_with_prepared_client(prepared, method, params)
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
        self.notify(config, server_name.as_str(), method, params, cwd)
            .await
    }

    pub async fn notify_connected(
        &self,
        server_name: &str,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        let prepared = self
            .prepare_existing_connected_client_with_gate("notify_connected", server_name)
            .await?;
        self.notify_with_prepared_client(prepared, method, params)
            .await
    }

    pub async fn notify_connected_named(
        &self,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
    ) -> crate::Result<()> {
        self.notify_connected(server_name.as_str(), method, params)
            .await
    }

    pub async fn notify_typed<N: McpNotification>(
        &self,
        config: &Config,
        server_name: &str,
        params: Option<N::Params>,
        cwd: &Path,
    ) -> crate::Result<()> {
        let params = crate::mcp::serialize_notification_params::<N>(server_name, params)?;
        self.notify(config, server_name, N::METHOD, params, cwd)
            .await
    }

    pub async fn notify_typed_named<N: McpNotification>(
        &self,
        config: &Config,
        server_name: &ServerName,
        params: Option<N::Params>,
        cwd: &Path,
    ) -> crate::Result<()> {
        self.notify_typed::<N>(config, server_name.as_str(), params, cwd)
            .await
    }

    pub async fn notify_typed_connected<N: McpNotification>(
        &self,
        server_name: &str,
        params: Option<N::Params>,
    ) -> crate::Result<()> {
        let params = crate::mcp::serialize_notification_params::<N>(server_name, params)?;
        self.notify_connected(server_name, N::METHOD, params).await
    }

    pub async fn notify_typed_connected_named<N: McpNotification>(
        &self,
        server_name: &ServerName,
        params: Option<N::Params>,
    ) -> crate::Result<()> {
        self.notify_typed_connected::<N>(server_name.as_str(), params)
            .await
    }
}
