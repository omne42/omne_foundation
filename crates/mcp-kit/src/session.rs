use std::time::Duration;

use anyhow::Context;
use serde_json::Value;

use crate::mcp::{
    CallToolRequest, CallToolRequestParams, CompleteRequest, GetPromptRequest,
    GetPromptRequestParams, ListPromptsRequest, ListResourceTemplatesRequest, ListResourcesRequest,
    ListToolsRequest, PingRequest, ReadResourceRequest, ReadResourceRequestParams, SetLevelRequest,
    SetLevelRequestParams, SubscribeRequest, SubscribeRequestParams, UnsubscribeRequest,
    UnsubscribeRequestParams, serialize_request_params,
};
use crate::{Connection, McpNotification, McpRequest, ServerName};

pub struct Session {
    server_name: ServerName,
    initialize_result: Value,
    connection: Connection,
    request_timeout: Duration,
}

impl Session {
    pub fn new(
        server_name: ServerName,
        connection: Connection,
        initialize_result: Value,
        request_timeout: Duration,
    ) -> Self {
        Self {
            server_name,
            initialize_result,
            connection,
            request_timeout,
        }
    }

    pub fn server_name(&self) -> &str {
        self.server_name.as_str()
    }

    pub fn server_name_named(&self) -> &ServerName {
        &self.server_name
    }

    pub fn initialize_result(&self) -> &Value {
        &self.initialize_result
    }

    /// Returns the underlying connection.
    ///
    /// This is an escape hatch for advanced use cases. Prefer `Session::{request,notify}` unless
    /// you specifically need access to the underlying JSON-RPC client or child process handle.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Returns the underlying connection (mutable).
    ///
    /// Warning: mutating the underlying client can bypass `Session`'s timeout wrappers and can
    /// make lifecycle/ownership less clear. If you take over lifecycle control, you are
    /// responsible for closing and waiting for any associated child process.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Consumes this session and returns the underlying connection.
    ///
    /// After calling this, the caller owns the connection lifecycle. In particular, if the
    /// connection was created via a `transport=stdio` server, you should call
    /// `Connection::wait_with_timeout` (or equivalent) to avoid leaving a child process running.
    pub fn into_connection(self) -> Connection {
        self.connection
    }

    /// Closes this session and (if present) waits for the underlying child process to exit.
    ///
    /// Note: this can hang indefinitely if the child does not exit. Prefer
    /// `Session::wait_with_timeout` if you need an upper bound.
    pub async fn wait(self) -> crate::Result<Option<std::process::ExitStatus>> {
        let Session {
            server_name,
            connection,
            ..
        } = self;
        connection
            .wait()
            .await
            .with_context(|| format!("close session (server={server_name})"))
            .map_err(crate::Error::from)
    }

    /// Closes this session and waits for the underlying child process to exit, up to `timeout`.
    ///
    /// This requires a Tokio runtime with the time driver enabled.
    pub async fn wait_with_timeout(
        self,
        timeout: Duration,
        on_timeout: mcp_jsonrpc::WaitOnTimeout,
    ) -> crate::Result<Option<std::process::ExitStatus>> {
        let Session {
            server_name,
            connection,
            ..
        } = self;
        connection
            .wait_with_timeout(timeout, on_timeout)
            .await
            .with_context(|| format!("close session (server={server_name})"))
            .map_err(crate::Error::from)
    }

    /// Override the per-request timeout used by `Session::{request,notify}`.
    ///
    /// Timeouts require a Tokio runtime with the time driver enabled.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub async fn request(&self, method: &str, params: Option<Value>) -> crate::Result<Value> {
        let result = self
            .connection
            .client()
            .request_optional_with_timeout(method, params, self.request_timeout)
            .await;
        match result {
            Ok(value) => Ok(value),
            Err(err) if err.is_wait_timeout() => Err(anyhow::Error::new(err)
                .context(format!(
                    "mcp request timed out after {:?}: {method} (server={})",
                    self.request_timeout, self.server_name
                ))
                .into()),
            Err(err) => Err(anyhow::Error::new(err)
                .context(format!(
                    "mcp request failed: {method} (server={})",
                    self.server_name
                ))
                .into()),
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> crate::Result<()> {
        crate::manager::ensure_tokio_time_driver("Session::notify")?;
        let timeout = self.request_timeout;
        let outcome = tokio::time::timeout(
            self.request_timeout,
            self.connection.client().notify(method, params),
        )
        .await;
        match outcome {
            Ok(result) => result
                .with_context(|| {
                    format!(
                        "mcp notification failed: {method} (server={})",
                        self.server_name
                    )
                })
                .map_err(crate::Error::from),
            Err(_) => {
                let timeout_message = format!(
                    "mcp notification timed out after {timeout:?}: {method} (server={})",
                    self.server_name
                );

                // Best-effort close: schedule only once so repeated timeout calls do not spawn
                // unbounded close tasks, and abort the client's background reader/transport
                // tasks so the timed-out session does not stay half-open.
                self.connection
                    .client()
                    .close_in_background_once(timeout_message.clone());
                Err(anyhow::Error::new(mcp_jsonrpc::Error::protocol(
                    mcp_jsonrpc::ProtocolErrorKind::WaitTimeout,
                    timeout_message,
                ))
                .into())
            }
        }
    }

    pub async fn request_typed<R: McpRequest>(
        &self,
        params: Option<R::Params>,
    ) -> crate::Result<R::Result> {
        let params = crate::mcp::serialize_request_params::<R>(self.server_name.as_str(), params)?;
        let result = self.request(R::METHOD, params).await?;
        crate::mcp::deserialize_request_result::<R>(self.server_name.as_str(), result)
    }

    pub async fn notify_typed<N: McpNotification>(
        &self,
        params: Option<N::Params>,
    ) -> crate::Result<()> {
        let params =
            crate::mcp::serialize_notification_params::<N>(self.server_name.as_str(), params)?;
        self.notify(N::METHOD, params).await
    }

    pub async fn ping(&self) -> crate::Result<Value> {
        self.request(PingRequest::METHOD, None).await
    }

    pub async fn list_tools(&self) -> crate::Result<Value> {
        self.request(ListToolsRequest::METHOD, None).await
    }

    pub async fn list_resources(&self) -> crate::Result<Value> {
        self.request(ListResourcesRequest::METHOD, None).await
    }

    pub async fn list_resource_templates(&self) -> crate::Result<Value> {
        self.request(ListResourceTemplatesRequest::METHOD, None)
            .await
    }

    pub async fn read_resource(&self, uri: &str) -> crate::Result<Value> {
        let params = serialize_request_params::<ReadResourceRequest>(
            self.server_name.as_str(),
            Some(ReadResourceRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request(ReadResourceRequest::METHOD, params).await
    }

    pub async fn subscribe_resource(&self, uri: &str) -> crate::Result<Value> {
        let params = serialize_request_params::<SubscribeRequest>(
            self.server_name.as_str(),
            Some(SubscribeRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request(SubscribeRequest::METHOD, params).await
    }

    pub async fn unsubscribe_resource(&self, uri: &str) -> crate::Result<Value> {
        let params = serialize_request_params::<UnsubscribeRequest>(
            self.server_name.as_str(),
            Some(UnsubscribeRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request(UnsubscribeRequest::METHOD, params).await
    }

    pub async fn list_prompts(&self) -> crate::Result<Value> {
        self.request(ListPromptsRequest::METHOD, None).await
    }

    pub async fn get_prompt(&self, prompt: &str, arguments: Option<Value>) -> crate::Result<Value> {
        let params = serialize_request_params::<GetPromptRequest>(
            self.server_name.as_str(),
            Some(GetPromptRequestParams {
                name: prompt.to_string(),
                arguments,
            }),
        )?;
        self.request(GetPromptRequest::METHOD, params).await
    }

    pub async fn call_tool(&self, tool: &str, arguments: Option<Value>) -> crate::Result<Value> {
        let params = serialize_request_params::<CallToolRequest>(
            self.server_name.as_str(),
            Some(CallToolRequestParams {
                name: tool.to_string(),
                arguments,
            }),
        )?;
        self.request(CallToolRequest::METHOD, params).await
    }

    pub async fn set_logging_level(&self, level: &str) -> crate::Result<Value> {
        let params = serialize_request_params::<SetLevelRequest>(
            self.server_name.as_str(),
            Some(SetLevelRequestParams {
                level: level.to_string(),
            }),
        )?;
        self.request(SetLevelRequest::METHOD, params).await
    }

    pub async fn complete(&self, params: Value) -> crate::Result<Value> {
        self.request(CompleteRequest::METHOD, Some(params)).await
    }
}
