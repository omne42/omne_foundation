use std::path::Path;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::mcp::{
    CallToolRequest, CallToolRequestParams, CompleteRequest, GetPromptRequest,
    GetPromptRequestParams, ListPromptsRequest, ListResourceTemplatesRequest, ListResourcesRequest,
    ListToolsRequest, PingRequest, ReadResourceRequest, ReadResourceRequestParams, SetLevelRequest,
    SetLevelRequestParams, SubscribeRequest, SubscribeRequestParams, UnsubscribeRequest,
    UnsubscribeRequestParams, serialize_request_params,
};
use crate::{
    Config, ErrorKind, Manager, McpNotification, McpRequest, ServerConfig, ServerName, Session,
};

use super::tagged_message;

impl Manager {
    pub async fn connect_session(
        &mut self,
        server_name: &str,
        server_cfg: &ServerConfig,
        cwd: &Path,
    ) -> crate::Result<Session> {
        self.connect(server_name, server_cfg, cwd).await?;
        Ok(self.take_session(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_session_named(
        &mut self,
        server_name: &ServerName,
        server_cfg: &ServerConfig,
        cwd: &Path,
    ) -> crate::Result<Session> {
        self.connect_named(server_name, server_cfg, cwd).await?;
        Ok(self.take_session_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_jsonrpc_session(
        &mut self,
        server_name: &str,
        client: mcp_jsonrpc::Client,
    ) -> crate::Result<Session> {
        self.connect_jsonrpc(server_name, client).await?;
        Ok(self.take_session(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_jsonrpc_session_named(
        &mut self,
        server_name: &ServerName,
        client: mcp_jsonrpc::Client,
    ) -> crate::Result<Session> {
        self.connect_jsonrpc(server_name.as_str(), client).await?;
        Ok(self.take_session_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_streamable_http_session(
        &mut self,
        server_name: &str,
        url: &str,
        http_options: mcp_jsonrpc::StreamableHttpOptions,
    ) -> crate::Result<Session> {
        self.connect_streamable_http(server_name, url, http_options)
            .await?;
        Ok(self.take_session(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_streamable_http_session_named(
        &mut self,
        server_name: &ServerName,
        url: &str,
        http_options: mcp_jsonrpc::StreamableHttpOptions,
    ) -> crate::Result<Session> {
        self.connect_streamable_http_named(server_name, url, http_options)
            .await?;
        Ok(self.take_session_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_streamable_http_split_session(
        &mut self,
        server_name: &str,
        sse_url: &str,
        post_url: &str,
        http_options: mcp_jsonrpc::StreamableHttpOptions,
    ) -> crate::Result<Session> {
        self.connect_streamable_http_split(server_name, sse_url, post_url, http_options)
            .await?;
        Ok(self.take_session(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_streamable_http_split_session_named(
        &mut self,
        server_name: &ServerName,
        sse_url: &str,
        post_url: &str,
        http_options: mcp_jsonrpc::StreamableHttpOptions,
    ) -> crate::Result<Session> {
        self.connect_streamable_http_split_named(server_name, sse_url, post_url, http_options)
            .await?;
        Ok(self.take_session_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_io_session<R, W>(
        &mut self,
        server_name: &str,
        read: R,
        write: W,
    ) -> crate::Result<Session>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        self.connect_io(server_name, read, write).await?;
        Ok(self.take_session(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn connect_io_session_named<R, W>(
        &mut self,
        server_name: &ServerName,
        read: R,
        write: W,
    ) -> crate::Result<Session>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        self.connect_io(server_name.as_str(), read, write).await?;
        Ok(self.take_session_named(server_name).ok_or_else(|| {
            tagged_message(
                ErrorKind::ManagerState,
                format!("mcp server not connected: {server_name}"),
            )
        })?)
    }

    pub async fn request(
        &mut self,
        config: &Config,
        server_name: &str,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.get_or_connect(config, server_name, cwd).await?;
        self.request_connected(server_name, method, params).await
    }

    pub async fn request_named(
        &mut self,
        config: &Config,
        server_name: &ServerName,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(config, server_name.as_str(), method, params, cwd)
            .await
    }

    pub async fn request_server(
        &mut self,
        server_name: &str,
        server_cfg: &ServerConfig,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.connect(server_name, server_cfg, cwd).await?;
        self.request_connected(server_name, method, params).await
    }

    pub async fn request_typed<R: McpRequest>(
        &mut self,
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

    pub async fn request_typed_connected<R: McpRequest>(
        &mut self,
        server_name: &str,
        params: Option<R::Params>,
    ) -> crate::Result<R::Result> {
        let params = crate::mcp::serialize_request_params::<R>(server_name, params)?;
        let result = self
            .request_connected(server_name, R::METHOD, params)
            .await?;
        crate::mcp::deserialize_request_result::<R>(server_name, result)
    }

    pub async fn notify(
        &mut self,
        config: &Config,
        server_name: &str,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<()> {
        self.get_or_connect(config, server_name, cwd).await?;
        self.notify_connected(server_name, method, params).await
    }

    pub async fn notify_server(
        &mut self,
        server_name: &str,
        server_cfg: &ServerConfig,
        method: &str,
        params: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<()> {
        self.connect(server_name, server_cfg, cwd).await?;
        self.notify_connected(server_name, method, params).await
    }

    pub async fn notify_typed<N: McpNotification>(
        &mut self,
        config: &Config,
        server_name: &str,
        params: Option<N::Params>,
        cwd: &Path,
    ) -> crate::Result<()> {
        let params = crate::mcp::serialize_notification_params::<N>(server_name, params)?;
        self.notify(config, server_name, N::METHOD, params, cwd)
            .await
    }

    pub async fn notify_typed_connected<N: McpNotification>(
        &mut self,
        server_name: &str,
        params: Option<N::Params>,
    ) -> crate::Result<()> {
        let params = crate::mcp::serialize_notification_params::<N>(server_name, params)?;
        self.notify_connected(server_name, N::METHOD, params).await
    }

    pub async fn list_tools(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(config, server_name, ListToolsRequest::METHOD, None, cwd)
            .await
    }

    pub async fn list_resources(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(config, server_name, ListResourcesRequest::METHOD, None, cwd)
            .await
    }

    pub async fn list_resource_templates(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(
            config,
            server_name,
            ListResourceTemplatesRequest::METHOD,
            None,
            cwd,
        )
        .await
    }

    pub async fn read_resource(
        &mut self,
        config: &Config,
        server_name: &str,
        uri: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<ReadResourceRequest>(
            server_name,
            Some(ReadResourceRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request(
            config,
            server_name,
            ReadResourceRequest::METHOD,
            params,
            cwd,
        )
        .await
    }

    pub async fn subscribe_resource(
        &mut self,
        config: &Config,
        server_name: &str,
        uri: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<SubscribeRequest>(
            server_name,
            Some(SubscribeRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request(config, server_name, SubscribeRequest::METHOD, params, cwd)
            .await
    }

    pub async fn unsubscribe_resource(
        &mut self,
        config: &Config,
        server_name: &str,
        uri: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<UnsubscribeRequest>(
            server_name,
            Some(UnsubscribeRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request(config, server_name, UnsubscribeRequest::METHOD, params, cwd)
            .await
    }

    pub async fn list_prompts(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(config, server_name, ListPromptsRequest::METHOD, None, cwd)
            .await
    }

    pub async fn get_prompt(
        &mut self,
        config: &Config,
        server_name: &str,
        prompt: &str,
        arguments: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<GetPromptRequest>(
            server_name,
            Some(GetPromptRequestParams {
                name: prompt.to_string(),
                arguments,
            }),
        )?;
        self.request(config, server_name, GetPromptRequest::METHOD, params, cwd)
            .await
    }

    pub async fn call_tool(
        &mut self,
        config: &Config,
        server_name: &str,
        tool: &str,
        arguments: Option<Value>,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<CallToolRequest>(
            server_name,
            Some(CallToolRequestParams {
                name: tool.to_string(),
                arguments,
            }),
        )?;
        self.request(config, server_name, CallToolRequest::METHOD, params, cwd)
            .await
    }

    pub async fn ping(
        &mut self,
        config: &Config,
        server_name: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(config, server_name, PingRequest::METHOD, None, cwd)
            .await
    }

    pub async fn set_logging_level(
        &mut self,
        config: &Config,
        server_name: &str,
        level: &str,
        cwd: &Path,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<SetLevelRequest>(
            server_name,
            Some(SetLevelRequestParams {
                level: level.to_string(),
            }),
        )?;
        self.request(config, server_name, SetLevelRequest::METHOD, params, cwd)
            .await
    }

    pub async fn complete(
        &mut self,
        config: &Config,
        server_name: &str,
        params: Value,
        cwd: &Path,
    ) -> crate::Result<Value> {
        self.request(
            config,
            server_name,
            CompleteRequest::METHOD,
            Some(params),
            cwd,
        )
        .await
    }

    pub async fn list_tools_connected(&mut self, server_name: &str) -> crate::Result<Value> {
        self.request_connected(server_name, ListToolsRequest::METHOD, None)
            .await
    }

    pub async fn list_resources_connected(&mut self, server_name: &str) -> crate::Result<Value> {
        self.request_connected(server_name, ListResourcesRequest::METHOD, None)
            .await
    }

    pub async fn list_resource_templates_connected(
        &mut self,
        server_name: &str,
    ) -> crate::Result<Value> {
        self.request_connected(server_name, ListResourceTemplatesRequest::METHOD, None)
            .await
    }

    pub async fn read_resource_connected(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<ReadResourceRequest>(
            server_name,
            Some(ReadResourceRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request_connected(server_name, ReadResourceRequest::METHOD, params)
            .await
    }

    pub async fn subscribe_resource_connected(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<SubscribeRequest>(
            server_name,
            Some(SubscribeRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request_connected(server_name, SubscribeRequest::METHOD, params)
            .await
    }

    pub async fn unsubscribe_resource_connected(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<UnsubscribeRequest>(
            server_name,
            Some(UnsubscribeRequestParams {
                uri: uri.to_string(),
            }),
        )?;
        self.request_connected(server_name, UnsubscribeRequest::METHOD, params)
            .await
    }

    pub async fn list_prompts_connected(&mut self, server_name: &str) -> crate::Result<Value> {
        self.request_connected(server_name, ListPromptsRequest::METHOD, None)
            .await
    }

    pub async fn get_prompt_connected(
        &mut self,
        server_name: &str,
        prompt: &str,
        arguments: Option<Value>,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<GetPromptRequest>(
            server_name,
            Some(GetPromptRequestParams {
                name: prompt.to_string(),
                arguments,
            }),
        )?;
        self.request_connected(server_name, GetPromptRequest::METHOD, params)
            .await
    }

    pub async fn ping_connected(&mut self, server_name: &str) -> crate::Result<Value> {
        self.request_connected(server_name, PingRequest::METHOD, None)
            .await
    }

    pub async fn set_logging_level_connected(
        &mut self,
        server_name: &str,
        level: &str,
    ) -> crate::Result<Value> {
        let params = serialize_request_params::<SetLevelRequest>(
            server_name,
            Some(SetLevelRequestParams {
                level: level.to_string(),
            }),
        )?;
        self.request_connected(server_name, SetLevelRequest::METHOD, params)
            .await
    }

    pub async fn complete_connected(
        &mut self,
        server_name: &str,
        params: Value,
    ) -> crate::Result<Value> {
        self.request_connected(server_name, CompleteRequest::METHOD, Some(params))
            .await
    }
}
