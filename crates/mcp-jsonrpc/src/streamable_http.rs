use std::borrow::Cow;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use http_kit::{
    HttpClientOptions, HttpClientProfile, body_preview_json, build_http_client_profile,
    drain_response_body, read_response_body_preview_text, redact_reqwest_error,
};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

use crate::{Client, ClientHandle, Error, ProtocolErrorKind, SpawnOptions, StreamableHttpOptions};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SseWakeReason {
    Connect,
    SessionChanged,
}

fn ends_with_ignore_ascii_case(haystack: &str, suffix: &str) -> bool {
    if suffix.len() > haystack.len() {
        return false;
    }
    haystack
        .get(haystack.len() - suffix.len()..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

fn media_type(content_type: &str) -> &str {
    content_type.trim().split(';').next().unwrap_or("").trim()
}

fn is_event_stream_content_type(content_type: &str) -> bool {
    media_type(content_type).eq_ignore_ascii_case("text/event-stream")
}

fn is_json_content_type(content_type: &str) -> bool {
    if content_type.trim().is_empty() {
        return true;
    }
    let ct = media_type(content_type);
    let Some((ty, subty)) = ct.split_once('/') else {
        return false;
    };
    if !ty.eq_ignore_ascii_case("application") {
        return false;
    }
    if subty.eq_ignore_ascii_case("json") {
        return true;
    }
    ends_with_ignore_ascii_case(subty, "+json")
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JsonRpcIdProbe {
    Object(JsonRpcIdObjectProbe),
    Other(IgnoredAny),
}

#[derive(Default)]
struct JsonRpcIdObjectProbe {
    id: Option<Value>,
    saw_id: bool,
}

impl<'de> Deserialize<'de> for JsonRpcIdObjectProbe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ProbeVisitor;

        impl<'de> Visitor<'de> for ProbeVisitor {
            type Value = JsonRpcIdObjectProbe;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut probe = JsonRpcIdObjectProbe::default();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if key == "id" {
                        probe.saw_id = true;
                        probe.id = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(probe)
            }
        }

        deserializer.deserialize_map(ProbeVisitor)
    }
}

fn jsonrpc_response_id_from_line(line: &[u8]) -> Result<Option<Value>, serde_json::Error> {
    let id = match serde_json::from_slice::<JsonRpcIdProbe>(line)? {
        JsonRpcIdProbe::Object(probe) if probe.saw_id => probe.id,
        JsonRpcIdProbe::Other(_) | JsonRpcIdProbe::Object(_) => None,
    };
    Ok(match id {
        Some(Value::String(id)) => Some(Value::String(id)),
        Some(Value::Number(id)) => Some(Value::Number(id)),
        Some(Value::Null) => Some(Value::Null),
        _ => None,
    })
}

fn jsonrpc_request_id_from_line(line: &[u8]) -> Result<Option<crate::Id>, serde_json::Error> {
    Ok(match jsonrpc_response_id_from_line(line)? {
        Some(Value::String(id)) => Some(crate::Id::String(id)),
        Some(Value::Number(id)) => id
            .as_i64()
            .map(crate::Id::Integer)
            .or_else(|| id.as_u64().map(crate::Id::Unsigned)),
        _ => None,
    })
}

const SSE_EVENT_BUFFER_RETAIN_BYTES: usize = 64 * 1024;
const HTTP_RESPONSE_INITIAL_CAP_BYTES: usize = 64 * 1024;
const HTTP_RESPONSE_UNKNOWN_LENGTH_INITIAL_CAP_BYTES: usize = 4 * 1024;

impl Client {
    pub async fn connect_streamable_http(url: &str) -> Result<Self, Error> {
        Self::connect_streamable_http_with_options(
            url,
            StreamableHttpOptions::default(),
            SpawnOptions::default(),
        )
        .await
    }

    pub async fn connect_streamable_http_with_options(
        url: &str,
        http_options: StreamableHttpOptions,
        options: SpawnOptions,
    ) -> Result<Self, Error> {
        Self::connect_streamable_http_split_with_options(url, url, http_options, options).await
    }

    pub async fn connect_streamable_http_split_with_options(
        sse_url: &str,
        post_url: &str,
        http_options: StreamableHttpOptions,
        options: SpawnOptions,
    ) -> Result<Self, Error> {
        async fn try_connect_sse(
            http_client: &reqwest::Client,
            sse_url: &str,
            connect_timeout: Option<Duration>,
            session_id: &Arc<tokio::sync::Mutex<Option<String>>>,
        ) -> Result<Option<reqwest::Response>, Error> {
            let mut req = http_client
                .get(sse_url)
                .header(reqwest::header::ACCEPT, "text/event-stream");
            {
                let guard = session_id.lock().await;
                if let Some(session) = guard.as_deref() {
                    req = req.header("mcp-session-id", session);
                }
            }

            let send = req.send();
            let resp = match connect_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, send).await {
                    Ok(resp) => resp,
                    Err(_) => {
                        return Err(Error::protocol(
                            ProtocolErrorKind::StreamableHttp,
                            "connect streamable http failed: request timed out",
                        ));
                    }
                },
                None => send.await,
            }
            .map_err(|err| {
                Error::protocol(
                    ProtocolErrorKind::StreamableHttp,
                    format!(
                        "connect streamable http failed: {}",
                        redact_reqwest_error(&err)
                    ),
                )
            })?;

            if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
                return Ok(None);
            }

            if !resp.status().is_success() {
                return Err(Error::protocol(
                    ProtocolErrorKind::StreamableHttp,
                    format!(
                        "streamable http SSE connect failed: status={}",
                        resp.status()
                    ),
                ));
            }

            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !is_event_stream_content_type(content_type) {
                return Err(Error::protocol(
                    ProtocolErrorKind::StreamableHttp,
                    format!(
                        "streamable http SSE connect failed: expected content-type text/event-stream, got {content_type}"
                    ),
                ));
            }

            if let Some(value) = resp.headers().get("mcp-session-id") {
                if let Ok(value) = value.to_str() {
                    let mut guard = session_id.lock().await;
                    if guard.as_deref() != Some(value) {
                        *guard = Some(value.to_owned());
                    }
                }
            }

            Ok(Some(resp))
        }

        let max_message_bytes =
            crate::normalize_max_message_bytes(options.limits.max_message_bytes);
        let connect_timeout = http_options.connect_timeout;
        let request_timeout = http_options.request_timeout;
        let follow_redirects = http_options.follow_redirects;
        let error_body_preview_bytes = http_options.error_body_preview_bytes;
        let enforce_public_ip = http_options.enforce_public_ip;
        if connect_timeout.is_some() || request_timeout.is_some() {
            crate::ensure_tokio_time_driver("Client::connect_streamable_http*_with_options")?;
        }

        let mut headers = reqwest::header::HeaderMap::new();
        for (key, value) in http_options.headers {
            let name = reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|_| {
                Error::protocol(
                    ProtocolErrorKind::InvalidInput,
                    format!("invalid http header name: {key}"),
                )
            })?;
            let value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
                Error::protocol(
                    ProtocolErrorKind::InvalidInput,
                    format!("invalid http header value: {key}"),
                )
            })?;
            headers.insert(name, value);
        }

        let http_options = HttpClientOptions {
            connect_timeout,
            default_headers: headers,
            follow_redirects,
            // Avoid automatic proxy environment variable loading by default.
            no_proxy: true,
            ..Default::default()
        };

        let http_client_profile = build_http_client_profile(&http_options).map_err(|err| {
            Error::protocol(
                ProtocolErrorKind::InvalidInput,
                format!("build http client profile failed: {err}"),
            )
        })?;
        let sse_url = reqwest::Url::parse(sse_url).map_err(|err| {
            Error::protocol(
                ProtocolErrorKind::InvalidInput,
                format!("invalid sse url: {err}"),
            )
        })?;
        let post_url = reqwest::Url::parse(post_url).map_err(|err| {
            Error::protocol(
                ProtocolErrorKind::InvalidInput,
                format!("invalid post url: {err}"),
            )
        })?;

        let (client_stream, bridge_stream) = tokio::io::duplex(1024 * 64);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (bridge_read, bridge_write) = tokio::io::split(bridge_stream);

        let mut client = Self::connect_io_with_options(client_read, client_write, options).await?;
        let transport_handle = client.handle.clone();

        let writer: Arc<tokio::sync::Mutex<_>> = Arc::new(tokio::sync::Mutex::new(bridge_write));
        let session_id: Arc<tokio::sync::Mutex<Option<String>>> =
            Arc::new(tokio::sync::Mutex::new(None));

        let (sse_wake_tx, sse_wake_rx) = mpsc::unbounded_channel::<SseWakeReason>();
        let sse_client = http_client_profile
            .select_for_url(&sse_url, enforce_public_ip)
            .await
            .map_err(|err| {
                Error::protocol(
                    ProtocolErrorKind::StreamableHttp,
                    format!("select http client failed: {err}"),
                )
            })?;
        let sse_resp =
            try_connect_sse(&sse_client, sse_url.as_str(), connect_timeout, &session_id).await?;

        let http_client_profile_post = http_client_profile.clone();
        let writer_post = writer.clone();
        let session_id_post = session_id.clone();
        let sse_wake_post = sse_wake_tx.clone();
        let request_timeout_post = request_timeout;
        let error_body_preview_bytes_post = error_body_preview_bytes;
        let handle_post = transport_handle.clone();
        let post_url_post = post_url.clone();
        let post_task = tokio::spawn(async move {
            HttpPostBridge {
                bridge_read,
                writer: writer_post,
                handle: handle_post,
                http_client_profile: http_client_profile_post,
                post_url: post_url_post,
                session_id: session_id_post,
                sse_wake: sse_wake_post,
                max_message_bytes,
                request_timeout: request_timeout_post,
                error_body_preview_bytes: error_body_preview_bytes_post,
                enforce_public_ip,
            }
            .run()
            .await;
        });

        let writer_sse = writer.clone();
        let session_id_sse = session_id.clone();
        let sse_url = sse_url.clone();
        let http_client_profile_sse = http_client_profile.clone();
        let handle_sse = transport_handle;
        let sse_task = tokio::spawn(async move {
            let mut wake_rx = sse_wake_rx;
            let mut current_resp = sse_resp;

            loop {
                let resp = match current_resp.take() {
                    Some(resp) => resp,
                    None => {
                        let Some(_) = wake_rx.recv().await else {
                            return;
                        };
                        let sse_client = match http_client_profile_sse
                            .select_for_url(&sse_url, enforce_public_ip)
                            .await
                        {
                            Ok(client) => client,
                            Err(err) => {
                                close_post_bridge(
                                    &writer_sse,
                                    &handle_sse,
                                    format!("streamable http SSE client selection failed: {err}"),
                                )
                                .await;
                                return;
                            }
                        };
                        match try_connect_sse(
                            &sse_client,
                            sse_url.as_str(),
                            connect_timeout,
                            &session_id_sse,
                        )
                        .await
                        {
                            Ok(Some(resp)) => resp,
                            Ok(None) => continue,
                            Err(err) => {
                                close_post_bridge(
                                    &writer_sse,
                                    &handle_sse,
                                    format!("streamable http SSE connection failed: {err}"),
                                )
                                .await;
                                return;
                            }
                        }
                    }
                };

                let reconnect = {
                    let writer = writer_sse.clone();
                    let mut pump_fut =
                        std::pin::pin!(pump_sse_response(resp, writer, max_message_bytes));

                    loop {
                        tokio::select! {
                            result = &mut pump_fut => match result {
                                Ok(()) => {
                                    // Treat graceful SSE EOF as a recoverable read-side event:
                                    // reconnect instead of tearing down the whole transport.
                                    break true;
                                }
                                Err(err) => {
                                    close_post_bridge(
                                        &writer_sse,
                                        &handle_sse,
                                        format!("streamable http SSE connection failed: {err}"),
                                    )
                                    .await;
                                    return;
                                }
                            },
                            wake = wake_rx.recv() => {
                                match wake {
                                    Some(SseWakeReason::SessionChanged) => {
                                        // Exiting this scope drops the stale SSE pump before the
                                        // reconnect path selects a new socket.
                                        break true;
                                    }
                                    Some(SseWakeReason::Connect) => {}
                                    None => {
                                        match pump_fut.await {
                                            Ok(()) => {
                                                close_post_bridge(
                                                    &writer_sse,
                                                    &handle_sse,
                                                    "streamable http SSE connection closed".to_string(),
                                                )
                                                .await;
                                            }
                                            Err(err) => {
                                                close_post_bridge(
                                                    &writer_sse,
                                                    &handle_sse,
                                                    format!("streamable http SSE connection failed: {err}"),
                                                )
                                                .await;
                                            }
                                        }
                                        return;
                                    }
                                }
                            }
                        }
                    }
                };

                if !reconnect {
                    continue;
                }

                let sse_client = match http_client_profile_sse
                    .select_for_url(&sse_url, enforce_public_ip)
                    .await
                {
                    Ok(client) => client,
                    Err(err) => {
                        close_post_bridge(
                            &writer_sse,
                            &handle_sse,
                            format!("streamable http SSE client selection failed: {err}"),
                        )
                        .await;
                        return;
                    }
                };
                current_resp = match try_connect_sse(
                    &sse_client,
                    sse_url.as_str(),
                    connect_timeout,
                    &session_id_sse,
                )
                .await
                {
                    Ok(Some(resp)) => Some(resp),
                    Ok(None) => None,
                    Err(err) => {
                        close_post_bridge(
                            &writer_sse,
                            &handle_sse,
                            format!("streamable http SSE connection failed: {err}"),
                        )
                        .await;
                        return;
                    }
                };
            }
        });

        client.transport_tasks.push(post_task);
        client.transport_tasks.push(sse_task);
        Ok(client)
    }
}

struct HttpPostBridge {
    bridge_read: tokio::io::ReadHalf<tokio::io::DuplexStream>,
    writer: Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    handle: ClientHandle,
    http_client_profile: HttpClientProfile,
    post_url: reqwest::Url,
    session_id: Arc<tokio::sync::Mutex<Option<String>>>,
    sse_wake: mpsc::UnboundedSender<SseWakeReason>,
    max_message_bytes: usize,
    request_timeout: Option<Duration>,
    error_body_preview_bytes: usize,
    enforce_public_ip: bool,
}

impl HttpPostBridge {
    async fn run(self) {
        const HTTP_TRANSPORT_ERROR: i64 = -32000;

        let Self {
            bridge_read,
            writer,
            handle,
            http_client_profile,
            post_url,
            session_id,
            sse_wake,
            max_message_bytes,
            request_timeout,
            error_body_preview_bytes,
            enforce_public_ip,
        } = self;

        let mut reader = tokio::io::BufReader::new(bridge_read);
        loop {
            let line = match crate::read_line_limited(&mut reader, max_message_bytes).await {
                Ok(Some(line)) => line,
                Ok(None) => return,
                Err(err) => {
                    close_post_bridge(
                        &writer,
                        &handle,
                        format!("streamable http POST bridge failed: {err}"),
                    )
                    .await;
                    return;
                }
            };
            if crate::is_ascii_whitespace_only(&line) {
                continue;
            }
            let line = Bytes::from(line);

            let selected_client = match http_client_profile
                .select_for_url(&post_url, enforce_public_ip)
                .await
            {
                Ok(client) => client,
                Err(err) => {
                    if !emit_post_bridge_error_from_line(
                        &writer,
                        &handle,
                        line.as_ref(),
                        HTTP_TRANSPORT_ERROR,
                        format!("http client selection failed: {err}"),
                        None,
                    )
                    .await
                    {
                        return;
                    }
                    continue;
                }
            };

            let mut req = selected_client
                .post(post_url.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "application/json, text/event-stream",
                )
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(line.clone());

            {
                let guard = session_id.lock().await;
                if let Some(session) = guard.as_deref() {
                    req = req.header("mcp-session-id", session);
                }
            }

            let send = req.send();
            let resp = match request_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, send).await {
                    Ok(resp) => resp,
                    Err(_) => {
                        if !emit_wait_timeout_from_line(
                            &writer,
                            &handle,
                            line.as_ref(),
                            "http request timed out".to_string(),
                        )
                        .await
                        {
                            return;
                        }
                        continue;
                    }
                },
                None => send.await,
            };
            let resp = match resp {
                Ok(resp) => resp,
                Err(err) => {
                    if !emit_post_bridge_error_from_line(
                        &writer,
                        &handle,
                        line.as_ref(),
                        HTTP_TRANSPORT_ERROR,
                        format!("http request failed: {}", redact_reqwest_error(&err)),
                        None,
                    )
                    .await
                    {
                        return;
                    }
                    continue;
                }
            };

            let mut wake_reason = if resp.status() == reqwest::StatusCode::ACCEPTED {
                Some(SseWakeReason::Connect)
            } else {
                None
            };
            if let Some(value) = resp.headers().get("mcp-session-id") {
                if let Ok(value) = value.to_str() {
                    let mut guard = session_id.lock().await;
                    let changed = guard.as_deref() != Some(value);
                    if changed {
                        *guard = Some(value.to_owned());
                    }
                    drop(guard);
                    if changed {
                        wake_reason = Some(SseWakeReason::SessionChanged);
                    }
                }
            }
            if let Some(reason) = wake_reason {
                let _ = sse_wake.send(reason);
            }

            let status = resp.status();
            if status.is_success() {
                let content_type = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                if is_event_stream_content_type(content_type) {
                    let stream = resp
                        .bytes_stream()
                        .map(|chunk| chunk.map_err(io::Error::other));
                    let reader = StreamReader::new(stream);
                    let mut reader = tokio::io::BufReader::new(reader);
                    let pump =
                        sse_pump_to_writer(&mut reader, &writer, max_message_bytes, true).await;
                    if pump.is_err()
                        && !emit_post_bridge_error_from_line(
                            &writer,
                            &handle,
                            line.as_ref(),
                            HTTP_TRANSPORT_ERROR,
                            "http response stream failed".to_string(),
                            None,
                        )
                        .await
                    {
                        return;
                    }
                    continue;
                }

                if !is_json_content_type(content_type) {
                    if !emit_post_bridge_error_from_line(
                        &writer,
                        &handle,
                        line.as_ref(),
                        HTTP_TRANSPORT_ERROR,
                        "unexpected content-type for json response".to_string(),
                        Some(serde_json::json!({ "content_type": content_type })),
                    )
                    .await
                    {
                        return;
                    }
                    continue;
                }

                let body = match request_timeout {
                    Some(timeout) => {
                        match tokio::time::timeout(
                            timeout,
                            read_response_body_limited(resp, max_message_bytes),
                        )
                        .await
                        {
                            Ok(body) => body,
                            Err(_) => {
                                if !emit_wait_timeout_from_line(
                                    &writer,
                                    &handle,
                                    line.as_ref(),
                                    "http response timed out".to_string(),
                                )
                                .await
                                {
                                    return;
                                }
                                continue;
                            }
                        }
                    }
                    None => read_response_body_limited(resp, max_message_bytes).await,
                };
                match body {
                    Err(ReadBodyError::TooLarge { actual_bytes }) => {
                        if !emit_post_bridge_error_from_line(
                            &writer,
                            &handle,
                            line.as_ref(),
                            HTTP_TRANSPORT_ERROR,
                            "http response too large".to_string(),
                            Some(serde_json::json!({
                                "max_bytes": max_message_bytes,
                                "actual_bytes": actual_bytes,
                            })),
                        )
                        .await
                        {
                            return;
                        }
                        continue;
                    }
                    Err(ReadBodyError::Http(err)) => {
                        if !emit_post_bridge_error_from_line(
                            &writer,
                            &handle,
                            line.as_ref(),
                            HTTP_TRANSPORT_ERROR,
                            format!("http response read failed: {}", redact_reqwest_error(&err)),
                            None,
                        )
                        .await
                        {
                            return;
                        }
                    }
                    Ok(body) if !body.is_empty() => {
                        // The JSON-RPC bridge is line-delimited. Re-serialize only when the HTTP
                        // body contains literal newlines so pretty-printed JSON remains valid.
                        if body.iter().any(|b| *b == b'\n' || *b == b'\r') {
                            let parsed_json: Value = match serde_json::from_slice(&body) {
                                Ok(json) => json,
                                Err(_) => {
                                    let data = body_preview_json(&body, error_body_preview_bytes);
                                    if !emit_post_bridge_error_from_line(
                                        &writer,
                                        &handle,
                                        line.as_ref(),
                                        HTTP_TRANSPORT_ERROR,
                                        "http response is not valid json".to_string(),
                                        data,
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                    continue;
                                }
                            };
                            let compact = match serde_json::to_vec(&parsed_json) {
                                Ok(compact) => compact,
                                Err(err) => {
                                    if !emit_post_bridge_error_from_line(
                                        &writer,
                                        &handle,
                                        line.as_ref(),
                                        HTTP_TRANSPORT_ERROR,
                                        format!("http response json re-serialize failed: {err}"),
                                        None,
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                    continue;
                                }
                            };
                            if let Err(err) = write_json_line(&writer, &compact).await {
                                close_post_bridge(
                                    &writer,
                                    &handle,
                                    format!(
                                        "streamable http POST bridge failed writing response: {err}"
                                    ),
                                )
                                .await;
                                return;
                            }
                        } else {
                            if serde_json::from_slice::<serde::de::IgnoredAny>(&body).is_err() {
                                let data = body_preview_json(&body, error_body_preview_bytes);
                                if !emit_post_bridge_error_from_line(
                                    &writer,
                                    &handle,
                                    line.as_ref(),
                                    HTTP_TRANSPORT_ERROR,
                                    "http response is not valid json".to_string(),
                                    data,
                                )
                                .await
                                {
                                    return;
                                }
                                continue;
                            }
                            if let Err(err) = write_json_line(&writer, &body).await {
                                close_post_bridge(
                                    &writer,
                                    &handle,
                                    format!(
                                        "streamable http POST bridge failed writing response: {err}"
                                    ),
                                )
                                .await;
                                return;
                            }
                        }
                    }
                    Ok(body) if body.is_empty() => {
                        let id = match jsonrpc_response_id_from_line(line.as_ref()) {
                            Ok(id) => id,
                            Err(err) => {
                                close_post_bridge(
                                    &writer,
                                    &handle,
                                    format!(
                                        "streamable http POST bridge received invalid JSON from client: {err}"
                                    ),
                                )
                                .await;
                                return;
                            }
                        };
                        if id.is_none() || status == reqwest::StatusCode::ACCEPTED {
                            continue;
                        }
                        if !emit_post_bridge_error(
                            &writer,
                            &handle,
                            id,
                            HTTP_TRANSPORT_ERROR,
                            "http response is empty".to_string(),
                            None,
                        )
                        .await
                        {
                            return;
                        }
                    }
                    _ => {}
                }
                continue;
            }

            let body_text = match request_timeout {
                Some(timeout) if error_body_preview_bytes == 0 => {
                    let drain = drain_response_body(resp);
                    let _ = tokio::time::timeout(timeout, drain).await; // pre-commit: allow-let-underscore
                    None
                }
                Some(timeout) => {
                    let read = read_response_body_preview_text(resp, error_body_preview_bytes);
                    tokio::time::timeout(timeout, read)
                        .await
                        .unwrap_or_default()
                }
                None => {
                    // Without a request timeout, waiting for an error body to finish can hang
                    // indefinitely (e.g. keep-alive response without Content-Length).
                    None
                }
            };
            if !emit_post_bridge_error_from_line(
                &writer,
                &handle,
                line.as_ref(),
                HTTP_TRANSPORT_ERROR,
                format!("http error: {status}"),
                body_text.map(|body| serde_json::json!({ "body": body })),
            )
            .await
            {
                return;
            }
        }
    }
}

async fn emit_post_bridge_error_from_line(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    handle: &ClientHandle,
    line: &[u8],
    code: i64,
    message: String,
    data: Option<Value>,
) -> bool {
    let id = match jsonrpc_response_id_from_line(line) {
        Ok(id) => id,
        Err(err) => {
            close_post_bridge(
                writer,
                handle,
                format!("streamable http POST bridge received invalid JSON from client: {err}"),
            )
            .await;
            return false;
        }
    };
    emit_post_bridge_error(writer, handle, id, code, message, data).await
}

async fn emit_wait_timeout_from_line(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    handle: &ClientHandle,
    line: &[u8],
    message: String,
) -> bool {
    let id = match jsonrpc_request_id_from_line(line) {
        Ok(id) => id,
        Err(err) => {
            close_post_bridge(
                writer,
                handle,
                format!("streamable http POST bridge received invalid JSON from client: {err}"),
            )
            .await;
            return false;
        }
    };

    let Some(id) = id else {
        close_post_bridge(
            writer,
            handle,
            format!("streamable http notification timed out: {message}"),
        )
        .await;
        return false;
    };

    if let Some(tx) = crate::lock_pending(&handle.pending).remove(&id) {
        let _ = tx.send(Err(Error::protocol(
            ProtocolErrorKind::WaitTimeout,
            message,
        )));
    }
    true
}

async fn close_post_bridge(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    handle: &ClientHandle,
    reason: impl Into<String>,
) {
    handle.close_with_reason(reason.into()).await;
    let mut writer = writer.lock().await;
    let _ = writer.shutdown().await; // pre-commit: allow-let-underscore
    drop(writer);
}

async fn emit_post_bridge_error(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    handle: &ClientHandle,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> bool {
    if let Some(id) = id {
        if let Err(err) = write_error_response(writer, id, code, message, data).await {
            close_post_bridge(
                writer,
                handle,
                format!("streamable http POST bridge failed writing error response: {err}"),
            )
            .await;
            return false;
        }
        return true;
    }

    close_post_bridge(
        writer,
        handle,
        format!("streamable http notification failed: {message}"),
    )
    .await;
    false
}

enum ReadBodyError {
    Http(reqwest::Error),
    TooLarge { actual_bytes: usize },
}

async fn read_response_body_limited(
    resp: reqwest::Response,
    max_message_bytes: usize,
) -> Result<Vec<u8>, ReadBodyError> {
    let content_length = resp.content_length();
    let content_length_usize = content_length.and_then(|len| usize::try_from(len).ok());
    if let Some(actual_bytes) = content_length_usize {
        if actual_bytes > max_message_bytes {
            return Err(ReadBodyError::TooLarge { actual_bytes });
        }
    } else if content_length.is_some() {
        return Err(ReadBodyError::TooLarge {
            actual_bytes: usize::MAX,
        });
    }

    let initial_capacity = content_length_usize
        .map(|len| len.min(max_message_bytes))
        .unwrap_or_else(|| max_message_bytes.min(HTTP_RESPONSE_UNKNOWN_LENGTH_INITIAL_CAP_BYTES))
        .min(HTTP_RESPONSE_INITIAL_CAP_BYTES);
    let mut out = Vec::with_capacity(initial_capacity);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ReadBodyError::Http)?;
        let next_len = out.len().saturating_add(chunk.len());
        if next_len > max_message_bytes {
            let actual_bytes = content_length_usize.map_or(next_len, |len| len.max(next_len));
            return Err(ReadBodyError::TooLarge { actual_bytes });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

async fn pump_sse_response(
    resp: reqwest::Response,
    writer: Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    max_message_bytes: usize,
) -> io::Result<()> {
    let stream = resp
        .bytes_stream()
        .map(|chunk| chunk.map_err(io::Error::other));
    let reader = StreamReader::new(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    sse_pump_to_writer(&mut reader, &writer, max_message_bytes, false).await
}

async fn sse_pump_to_writer<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    max_message_bytes: usize,
    stop_on_done: bool,
) -> Result<(), io::Error> {
    let initial_capacity = max_message_bytes.min(4096);
    let mut data = Vec::with_capacity(initial_capacity);
    let mut line = Vec::with_capacity(initial_capacity);

    loop {
        if !crate::read_line_limited_into(reader, max_message_bytes, &mut line).await? {
            if flush_sse_event_data(writer, &mut data, max_message_bytes, stop_on_done).await? {
                return Ok(());
            }
            return Ok(());
        }

        if line.is_empty() {
            if flush_sse_event_data(writer, &mut data, max_message_bytes, stop_on_done).await? {
                return Ok(());
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix(b"data:") {
            // Per SSE parsing rules, only one optional U+0020 after ':' is stripped.
            let rest = rest.strip_prefix(b" ").unwrap_or(rest);

            if !data.is_empty() {
                data.push(b'\n');
            }
            if data.len().saturating_add(rest.len()) > max_message_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sse event too large",
                ));
            }
            data.extend_from_slice(rest);
        }
    }
}

async fn flush_sse_event_data(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    data: &mut Vec<u8>,
    max_message_bytes: usize,
    stop_on_done: bool,
) -> Result<bool, io::Error> {
    if data.is_empty() {
        return Ok(false);
    }

    if stop_on_done && data == b"[DONE]" {
        data.clear();
        return Ok(true);
    }

    let normalized = normalize_sse_event_data_for_json_line(data)?;
    write_json_line(writer, &normalized).await?;
    data.clear();
    if data.capacity() > SSE_EVENT_BUFFER_RETAIN_BYTES {
        let retain = SSE_EVENT_BUFFER_RETAIN_BYTES.min(max_message_bytes);
        data.shrink_to(retain);
    }
    Ok(false)
}

fn normalize_sse_event_data_for_json_line(data: &[u8]) -> Result<Vec<u8>, io::Error> {
    if data.iter().any(|byte| matches!(byte, b'\n' | b'\r')) {
        if let Ok(value) = serde_json::from_slice::<Value>(data) {
            return serde_json::to_vec(&value).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("serialize sse event payload failed: {err}"),
                )
            });
        }
    }

    Ok(data.to_vec())
}

async fn write_json_line(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    line: &[u8],
) -> Result<(), io::Error> {
    let mut writer = writer.lock().await;
    writer.write_all(line).await?;
    writer.write_all(b"\n").await?;
    drop(writer);
    Ok(())
}

async fn write_error_response(
    writer: &Arc<tokio::sync::Mutex<tokio::io::WriteHalf<tokio::io::DuplexStream>>>,
    id: Value,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<(), io::Error> {
    let mut error = serde_json::json!({
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        error["data"] = data;
    }
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    });

    let mut out = serde_json::to_vec(&response).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("serialize error response failed: {err}"),
        )
    })?;
    out.push(b'\n');

    let mut writer = writer.lock().await;
    writer.write_all(&out).await?;
    drop(writer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    #[test]
    fn jsonrpc_response_id_from_line_accepts_object_and_ignores_non_object() {
        let id = jsonrpc_response_id_from_line(br#"{"jsonrpc":"2.0","id":"abc","method":"x"}"#)
            .expect("valid json");
        assert_eq!(id, Some(serde_json::json!("abc")));

        let id = jsonrpc_response_id_from_line(br#"{"jsonrpc":"2.0","id":{"nested":1}}"#)
            .expect("valid json");
        assert!(id.is_none());

        let id = jsonrpc_response_id_from_line(br#"[{"id":1}]"#).expect("valid json");
        assert!(id.is_none());
    }

    #[test]
    fn jsonrpc_response_id_rejects_non_scalar_ids() {
        let id_string = jsonrpc_response_id_from_line(br#"{"id":"abc"}"#).expect("valid json");
        assert_eq!(id_string, Some(serde_json::json!("abc")));

        let id_number = jsonrpc_response_id_from_line(br#"{"id":7}"#).expect("valid json");
        assert_eq!(id_number, Some(serde_json::json!(7)));

        let id_null = jsonrpc_response_id_from_line(br#"{"id":null}"#).expect("valid json");
        assert_eq!(id_null, Some(serde_json::Value::Null));

        let id_object = jsonrpc_response_id_from_line(br#"{"id":{"x":1}}"#).expect("valid json");
        assert!(id_object.is_none());

        let missing = jsonrpc_response_id_from_line(br#"{"method":"ping"}"#).expect("valid json");
        assert!(missing.is_none());
    }

    #[test]
    fn jsonrpc_request_id_accepts_large_unsigned_numeric_ids() {
        let payload = format!(r#"{{"jsonrpc":"2.0","id":{}}}"#, u64::MAX);
        let id = jsonrpc_request_id_from_line(payload.as_bytes()).expect("valid json");
        assert_eq!(id, Some(crate::Id::Unsigned(u64::MAX)));
    }

    #[test]
    fn content_type_helpers_handle_common_variants() {
        assert!(is_event_stream_content_type("text/event-stream"));
        assert!(is_event_stream_content_type("Text/Event-Stream"));
        assert!(is_event_stream_content_type(
            "text/event-stream; charset=utf-8"
        ));
        assert!(!is_event_stream_content_type("application/json"));

        assert!(is_json_content_type(""));
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("Application/Json; charset=utf-8"));
        assert!(is_json_content_type("application/problem+json"));
        assert!(is_json_content_type(
            "application/vnd.api+json; charset=utf-8"
        ));
        assert!(!is_json_content_type("text/plain"));
        assert!(!is_json_content_type("application/xml"));
        assert!(!is_json_content_type("application/jsonp"));
        assert!(!is_json_content_type("application/notjson+jsone"));
    }

    #[test]
    fn body_preview_text_is_bounded_by_max_bytes() {
        let body = b"abcdefghijklmnopqrstuvwxyz";
        let preview = http_kit::body_preview_text(body, 8).expect("preview available");
        assert_eq!(preview, "abcdefgh");
    }

    #[test]
    fn body_preview_json_returns_none_for_zero_limit() {
        let body = b"{\"large\":true}";
        assert!(body_preview_json(body, 0).is_none());
    }

    #[tokio::test]
    async fn sse_pump_writes_data_events_as_json_lines() {
        let sse = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\",\"params\":{}}\n",
            "\n",
        );

        let (mut in_write, in_read) = tokio::io::duplex(1024);
        let write_task = tokio::spawn(async move {
            in_write.write_all(sse.as_bytes()).await.unwrap();
            // Close input.
            drop(in_write);
        });
        let mut reader = tokio::io::BufReader::new(in_read);

        let (client_side, mut capture_side) = tokio::io::duplex(1024);
        let (read, write) = tokio::io::split(client_side);
        drop(read);
        let writer = Arc::new(tokio::sync::Mutex::new(write));

        sse_pump_to_writer(&mut reader, &writer, 1024, false)
            .await
            .unwrap();
        drop(writer);

        write_task.await.unwrap();

        let mut out = Vec::new();
        capture_side.read_to_end(&mut out).await.unwrap();
        assert_eq!(
            out,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\",\"params\":{}}\n"
        );
    }

    #[test]
    fn normalize_sse_event_data_compacts_multiline_json_into_single_line() {
        let normalized = normalize_sse_event_data_for_json_line(
            br#"{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "ok": true
  }
}"#,
        )
        .expect("multiline json should normalize");

        assert_eq!(
            normalized,
            br#"{"id":1,"jsonrpc":"2.0","result":{"ok":true}}"#
        );
    }

    #[tokio::test]
    async fn sse_pump_flushes_last_data_event_without_trailing_blank_line()
    -> Result<(), Box<dyn std::error::Error>> {
        let sse = "data: {\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\"}\n";

        let (mut in_write, in_read) = tokio::io::duplex(1024);
        in_write.write_all(sse.as_bytes()).await?;
        drop(in_write);
        let mut reader = tokio::io::BufReader::new(in_read);

        let (client_side, mut capture_side) = tokio::io::duplex(1024);
        let (read, write) = tokio::io::split(client_side);
        drop(read);
        let writer = Arc::new(tokio::sync::Mutex::new(write));

        sse_pump_to_writer(&mut reader, &writer, 1024, false)
            .await
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        drop(writer);

        let mut out = Vec::new();
        capture_side.read_to_end(&mut out).await?;
        assert_eq!(out, b"{\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\"}\n");
        Ok(())
    }

    #[tokio::test]
    async fn sse_pump_stop_on_done_ignores_eof_done_without_trailing_blank_line()
    -> Result<(), Box<dyn std::error::Error>> {
        let sse = "data: [DONE]\n";

        let (mut in_write, in_read) = tokio::io::duplex(1024);
        in_write.write_all(sse.as_bytes()).await?;
        drop(in_write);
        let mut reader = tokio::io::BufReader::new(in_read);

        let (client_side, mut capture_side) = tokio::io::duplex(1024);
        let (read, write) = tokio::io::split(client_side);
        drop(read);
        let writer = Arc::new(tokio::sync::Mutex::new(write));

        sse_pump_to_writer(&mut reader, &writer, 1024, true)
            .await
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        drop(writer);

        let mut out = Vec::new();
        capture_side.read_to_end(&mut out).await?;
        assert!(out.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn sse_pump_strips_only_one_optional_space_after_data_colon()
    -> Result<(), Box<dyn std::error::Error>> {
        let sse = "data:  {\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\"}\n\n";

        let (mut in_write, in_read) = tokio::io::duplex(1024);
        in_write.write_all(sse.as_bytes()).await?;
        drop(in_write);
        let mut reader = tokio::io::BufReader::new(in_read);

        let (client_side, mut capture_side) = tokio::io::duplex(1024);
        let (read, write) = tokio::io::split(client_side);
        drop(read);
        let writer = Arc::new(tokio::sync::Mutex::new(write));

        sse_pump_to_writer(&mut reader, &writer, 1024, false)
            .await
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        drop(writer);

        let mut out = Vec::new();
        capture_side.read_to_end(&mut out).await?;
        assert_eq!(out, b" {\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\"}\n");
        Ok(())
    }

    #[tokio::test]
    async fn streamable_http_reconnects_sse_after_session_rollover() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let base_url = format!("http://{addr}");
        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let (mut initial_sse, _) = listener.accept().await.expect("accept initial SSE");
            let initial_request = read_http_request(&mut initial_sse).await.expect("read GET");
            assert_eq!(initial_request.method, "GET");
            assert_eq!(initial_request.path, "/sse");
            assert_eq!(header(&initial_request, "mcp-session-id"), None);
            write_chunked_response_headers(
                &mut initial_sse,
                "200 OK",
                &[
                    ("content-type", "text/event-stream"),
                    ("connection", "close"),
                    ("mcp-session-id", "session-1"),
                ],
            )
            .await
            .expect("write initial SSE headers");
            stage_tx.send("initial-sse-open").expect("send stage");

            let (mut post_stream, _) = listener.accept().await.expect("accept POST");
            let post_request = read_http_request(&mut post_stream)
                .await
                .expect("read POST");
            assert_eq!(post_request.method, "POST");
            assert_eq!(post_request.path, "/post");
            assert_eq!(
                header(&post_request, "mcp-session-id"),
                Some("session-1".to_string())
            );
            assert!(
                String::from_utf8(post_request.body)
                    .expect("utf8 POST body")
                    .contains("\"method\":\"demo/trigger\""),
                "unexpected POST body"
            );
            write_fixed_response(
                &mut post_stream,
                "202 Accepted",
                &[("mcp-session-id", "session-2"), ("connection", "close")],
                b"",
            )
            .await
            .expect("write POST response");
            stage_tx.send("session-rolled").expect("send stage");
            drop(post_stream);

            let (mut rollover_sse, _) = listener.accept().await.expect("accept rollover SSE");
            let rollover_request = read_http_request(&mut rollover_sse)
                .await
                .expect("read rollover GET");
            assert_eq!(rollover_request.method, "GET");
            assert_eq!(rollover_request.path, "/sse");
            assert_eq!(
                header(&rollover_request, "mcp-session-id"),
                Some("session-2".to_string())
            );
            write_chunked_response_headers(
                &mut rollover_sse,
                "200 OK",
                &[
                    ("content-type", "text/event-stream"),
                    ("mcp-session-id", "session-2"),
                ],
            )
            .await
            .expect("write rollover SSE headers");
            stage_tx.send("rollover-sse-open").expect("send stage");
            write_chunk(
                &mut rollover_sse,
                b"data: {\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\",\"params\":{\"session\":\"session-2\"}}\n\n",
            )
            .await
            .expect("write SSE notification");
            finish_chunked_response(&mut rollover_sse)
                .await
                .expect("finish SSE response");
            stage_tx.send("rollover-sse-finished").expect("send stage");
        });

        let mut client = Client::connect_streamable_http_split_with_options(
            &format!("{base_url}/sse"),
            &format!("{base_url}/post"),
            StreamableHttpOptions::default(),
            SpawnOptions::default(),
        )
        .await
        .expect("connect streamable http");
        let mut notifications = client
            .take_notifications()
            .expect("take notifications receiver");

        client
            .notify("demo/trigger", None)
            .await
            .expect("send trigger notification");

        let rollover_stage = tokio::time::timeout(Duration::from_secs(2), stage_rx.recv())
            .await
            .expect("stage should arrive before timeout")
            .expect("stage channel open");
        assert_eq!(rollover_stage, "initial-sse-open");
        let rollover_stage = tokio::time::timeout(Duration::from_secs(2), stage_rx.recv())
            .await
            .expect("stage should arrive before timeout")
            .expect("stage channel open");
        assert_eq!(rollover_stage, "session-rolled");
        let rollover_stage = tokio::time::timeout(Duration::from_secs(2), stage_rx.recv())
            .await
            .expect("second SSE connect should happen before timeout")
            .expect("stage channel open");
        assert_eq!(rollover_stage, "rollover-sse-open");

        let notification = tokio::time::timeout(Duration::from_secs(2), notifications.recv())
            .await
            .expect("notification should arrive before timeout")
            .expect("notification stream open");
        assert_eq!(notification.method, "demo/notify");
        assert_eq!(
            notification.params,
            Some(serde_json::json!({ "session": "session-2" }))
        );
        let rollover_stage = tokio::time::timeout(Duration::from_secs(2), stage_rx.recv())
            .await
            .expect("rollover SSE should finish before timeout")
            .expect("stage channel open");
        assert_eq!(rollover_stage, "rollover-sse-finished");

        client.close("test complete").await;
        server.await.expect("server task should join");
    }

    #[tokio::test]
    async fn streamable_http_reconnects_after_graceful_sse_eof() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let base_url = format!("http://{addr}");
        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut initial_sse, _) = listener.accept().await.expect("accept initial SSE");
            let initial_request = read_http_request(&mut initial_sse).await.expect("read GET");
            assert_eq!(initial_request.method, "GET");
            assert_eq!(initial_request.path, "/sse");
            assert_eq!(header(&initial_request, "mcp-session-id"), None);
            write_chunked_response_headers(
                &mut initial_sse,
                "200 OK",
                &[
                    ("content-type", "text/event-stream"),
                    ("mcp-session-id", "session-1"),
                ],
            )
            .await
            .expect("write initial SSE headers");
            stage_tx.send("initial-sse-open").expect("send stage");
            finish_chunked_response(&mut initial_sse)
                .await
                .expect("finish initial SSE response");
            drop(initial_sse);

            let (mut reconnected_sse, _) = listener.accept().await.expect("accept reconnect SSE");
            let reconnect_request = read_http_request(&mut reconnected_sse)
                .await
                .expect("read reconnect GET");
            assert_eq!(reconnect_request.method, "GET");
            assert_eq!(reconnect_request.path, "/sse");
            assert_eq!(
                header(&reconnect_request, "mcp-session-id"),
                Some("session-1".to_string())
            );
            write_chunked_response_headers(
                &mut reconnected_sse,
                "200 OK",
                &[
                    ("content-type", "text/event-stream"),
                    ("mcp-session-id", "session-1"),
                ],
            )
            .await
            .expect("write reconnect SSE headers");
            stage_tx.send("reconnected-sse-open").expect("send stage");
            write_chunk(
                &mut reconnected_sse,
                b"data: {\"jsonrpc\":\"2.0\",\"method\":\"demo/notify\",\"params\":{\"session\":\"session-1\"}}\n\n",
            )
            .await
            .expect("write reconnect SSE notification");
            let _ = close_rx.await;
            drop(reconnected_sse);
        });

        let mut client = Client::connect_streamable_http_split_with_options(
            &format!("{base_url}/sse"),
            &format!("{base_url}/post"),
            StreamableHttpOptions::default(),
            SpawnOptions::default(),
        )
        .await
        .expect("connect streamable http");
        let mut notifications = client
            .take_notifications()
            .expect("take notifications receiver");

        let reconnect_timeout = Duration::from_secs(10);

        let stage = tokio::time::timeout(reconnect_timeout, stage_rx.recv())
            .await
            .expect("initial SSE stage should arrive before timeout")
            .expect("stage channel open");
        assert_eq!(stage, "initial-sse-open");
        let stage = tokio::time::timeout(reconnect_timeout, stage_rx.recv())
            .await
            .expect("reconnect SSE stage should arrive before timeout")
            .expect("stage channel open");
        assert_eq!(stage, "reconnected-sse-open");

        let notification = tokio::time::timeout(reconnect_timeout, notifications.recv())
            .await
            .expect("notification should arrive before timeout")
            .expect("notification stream open");
        assert_eq!(notification.method, "demo/notify");
        assert_eq!(
            notification.params,
            Some(serde_json::json!({ "session": "session-1" }))
        );

        client.close("test complete").await;
        let _ = close_tx.send(());
        server.await.expect("server task should join");
    }

    #[derive(Debug)]
    struct HttpRequest {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> io::Result<HttpRequest> {
        let mut buf = Vec::new();
        let header_end = loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await?;
            buf.push(byte[0]);
            if buf.len() >= 4 && buf[buf.len() - 4..] == *b"\r\n\r\n" {
                break buf.len() - 4;
            }
        };

        let header_text = std::str::from_utf8(&buf[..header_end]).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("http request header was not valid utf-8: {err}"),
            )
        })?;
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing http request line")
        })?;
        let mut request_line_parts = request_line.split_whitespace();
        let method = request_line_parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http method"))?
            .to_string();
        let path = request_line_parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http path"))?
            .to_string();

        let mut headers = BTreeMap::new();
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid http header line: {line}"),
                ));
            };
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }

        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0_u8; content_length];
        if content_length > 0 {
            stream.read_exact(&mut body).await?;
        }

        Ok(HttpRequest {
            method,
            path,
            headers,
            body,
        })
    }

    fn header(request: &HttpRequest, name: &str) -> Option<String> {
        request.headers.get(&name.to_ascii_lowercase()).cloned()
    }

    async fn write_fixed_response(
        stream: &mut tokio::net::TcpStream,
        status: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> io::Result<()> {
        let mut response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        stream.write_all(response.as_bytes()).await?;
        if !body.is_empty() {
            stream.write_all(body).await?;
        }
        stream.flush().await
    }

    async fn write_chunked_response_headers(
        stream: &mut tokio::net::TcpStream,
        status: &str,
        headers: &[(&str, &str)],
    ) -> io::Result<()> {
        let mut response = format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\n");
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        stream.write_all(response.as_bytes()).await?;
        stream.flush().await
    }

    async fn write_chunk(stream: &mut tokio::net::TcpStream, body: &[u8]) -> io::Result<()> {
        stream
            .write_all(format!("{:X}\r\n", body.len()).as_bytes())
            .await?;
        stream.write_all(body).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await
    }

    async fn finish_chunked_response(stream: &mut tokio::net::TcpStream) -> io::Result<()> {
        stream.write_all(b"0\r\n\r\n").await?;
        stream.flush().await
    }
}
