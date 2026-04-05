use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncRead};
use tokio::sync::mpsc;

use crate::stdout_log::LogState;
use crate::{
    BatchResponseWriter, CancelledRequestIds, ClientHandle, ClientStatsInner, DiagnosticsState,
    Error, IncomingRequest, Limits, Notification, PendingRequests, ProtocolErrorKind,
    RequestResponder, StdoutLogRedactor, error_response_id_or_null, handle_response,
    normalize_max_message_bytes, outbound_error_response_value, parse_id_owned,
};

const REUSABLE_LINE_BUFFER_RETAIN_BYTES: usize = 64 * 1024;
const READ_LINE_INITIAL_CAP_BYTES: usize = 4 * 1024;

pub(crate) struct ReaderTaskContext {
    pub(crate) pending: PendingRequests,
    pub(crate) cancelled_request_ids: CancelledRequestIds,
    pub(crate) stats: Arc<ClientStatsInner>,
    pub(crate) notify_tx: mpsc::Sender<Notification>,
    pub(crate) request_tx: mpsc::Sender<IncomingRequest>,
    pub(crate) responder: ClientHandle,
    pub(crate) stdout_log: Option<LogState>,
    pub(crate) stdout_log_redactor: Option<StdoutLogRedactor>,
    pub(crate) diagnostics_state: Option<Arc<DiagnosticsState>>,
    pub(crate) limits: Limits,
}

pub(crate) fn spawn_reader_task<R>(reader: R, ctx: ReaderTaskContext) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let ReaderTaskContext {
            pending,
            cancelled_request_ids,
            stats,
            notify_tx,
            request_tx,
            responder,
            stdout_log,
            stdout_log_redactor,
            diagnostics_state,
            limits,
        } = ctx;

        let mut log_state = stdout_log;

        let max_message_bytes = normalize_max_message_bytes(limits.max_message_bytes);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            let next = read_line_limited_into(&mut reader, max_message_bytes, &mut line).await;
            match next {
                Ok(true) => {
                    if is_ascii_whitespace_only(&line) {
                        continue;
                    }
                    if let Some(state) = &mut log_state {
                        let write_result = match &stdout_log_redactor {
                            Some(redactor) => state.write_line_bytes(&redactor(&line)).await,
                            None => state.write_line_bytes(&line).await,
                        };
                        if let Err(err) = write_result {
                            responder.record_stdout_log_write_error(&err);
                            log_state = None;
                        }
                    }
                    let value: Value = match serde_json::from_slice(&line) {
                        Ok(value) => value,
                        Err(_) => {
                            stats.invalid_json_lines.fetch_add(1, Ordering::Relaxed);
                            if let Some(diagnostics) = &diagnostics_state {
                                diagnostics.record_invalid_json_line(&line);
                            }
                            close_invalid_message(
                                &responder,
                                "peer sent invalid JSON line".to_string(),
                            )
                            .await;
                            return;
                        }
                    };
                    if let Err(reason) = handle_incoming_value(
                        value,
                        &pending,
                        &cancelled_request_ids,
                        &stats,
                        &notify_tx,
                        &request_tx,
                        &responder,
                    )
                    .await
                    {
                        close_invalid_message(&responder, reason).await;
                        return;
                    }
                    if responder.is_closed() {
                        return;
                    }
                }
                Ok(false) => {
                    responder
                        .close_with_reason("server closed connection")
                        .await;
                    return;
                }
                Err(err) => {
                    let reason = format!("io error: {err}");
                    responder.close_with_error(reason, Error::Io(err)).await;
                    return;
                }
            }
        }
    })
}

async fn close_invalid_message(responder: &ClientHandle, reason: String) {
    responder
        .close_with_error(
            reason.clone(),
            Error::protocol(ProtocolErrorKind::InvalidMessage, reason),
        )
        .await;
}

async fn handle_incoming_value(
    value: Value,
    pending: &PendingRequests,
    cancelled_request_ids: &CancelledRequestIds,
    stats: &Arc<ClientStatsInner>,
    notify_tx: &mpsc::Sender<Notification>,
    request_tx: &mpsc::Sender<IncomingRequest>,
    responder: &ClientHandle,
) -> Result<(), String> {
    const INVALID_REQUEST: i64 = -32600;
    let ctx = IncomingValueContext {
        pending,
        cancelled_request_ids,
        stats,
        notify_tx,
        request_tx,
        responder,
    };

    match value {
        Value::Array(items) => {
            if items.is_empty() {
                let _ = ctx
                    .responder
                    .respond_error_raw_id(Value::Null, INVALID_REQUEST, "empty batch", None)
                    .await;
                return Err("peer sent empty JSON-RPC batch".to_string());
            }

            let batch = BatchResponseWriter::new(ctx.responder.clone());
            let mut stack = Vec::with_capacity(items.len());
            stack.extend(items.into_iter().rev().map(|item| (item, false)));
            while let Some((item, allow_batch_expansion)) = stack.pop() {
                if let Err(reason) = handle_incoming_item(
                    item,
                    allow_batch_expansion,
                    &ctx,
                    Some(&batch),
                    &mut stack,
                )
                .await
                {
                    let _ = batch.finish().await;
                    return Err(reason);
                }
                if ctx.responder.is_closed() {
                    return Ok(());
                }
            }
            let _ = batch.finish().await;
            Ok(())
        }
        other => {
            let mut stack = Vec::new();
            handle_incoming_item(other, true, &ctx, None, &mut stack).await
        }
    }
}

async fn close_notification_queue_overflow(
    ctx: &IncomingValueContext<'_>,
    reason: String,
    counter: &AtomicU64,
) {
    counter.fetch_add(1, Ordering::Relaxed);
    ctx.responder
        .close_with_error(
            reason.clone(),
            Error::protocol(ProtocolErrorKind::Other, reason),
        )
        .await;
}

struct IncomingValueContext<'a> {
    pending: &'a PendingRequests,
    cancelled_request_ids: &'a CancelledRequestIds,
    stats: &'a Arc<ClientStatsInner>,
    notify_tx: &'a mpsc::Sender<Notification>,
    request_tx: &'a mpsc::Sender<IncomingRequest>,
    responder: &'a ClientHandle,
}

async fn handle_incoming_item(
    value: Value,
    allow_batch_expansion: bool,
    ctx: &IncomingValueContext<'_>,
    batch: Option<&BatchResponseWriter>,
    stack: &mut Vec<(Value, bool)>,
) -> Result<(), String> {
    const INVALID_REQUEST: i64 = -32600;

    match value {
        Value::Array(items) => {
            if !allow_batch_expansion {
                let _ = send_batch_or_direct_error_raw_id(
                    ctx.responder,
                    batch,
                    Value::Null,
                    INVALID_REQUEST,
                    "nested batch is not allowed",
                    None,
                )
                .await;
                return Err("peer sent nested JSON-RPC batch".to_string());
            }

            if items.is_empty() {
                let _ = send_batch_or_direct_error_raw_id(
                    ctx.responder,
                    batch,
                    Value::Null,
                    INVALID_REQUEST,
                    "empty batch",
                    None,
                )
                .await;
                return Err("peer sent empty nested JSON-RPC batch".to_string());
            }

            stack.reserve(items.len());
            stack.extend(items.into_iter().rev().map(|item| (item, false)));
            Ok(())
        }
        Value::Object(mut map) => {
            let jsonrpc_valid = map.get("jsonrpc").and_then(Value::as_str) == Some("2.0");

            match map.remove("method") {
                Some(Value::String(method)) => {
                    let id_value = map.remove("id");
                    if !jsonrpc_valid {
                        if let Some(id_value) = id_value {
                            let id_value = error_response_id_or_null(id_value);
                            let _ = send_batch_or_direct_error_raw_id(
                                ctx.responder,
                                batch,
                                id_value,
                                INVALID_REQUEST,
                                "invalid jsonrpc version",
                                None,
                            )
                            .await;
                        }
                        return Err(
                            "peer sent request/notification with invalid jsonrpc version"
                                .to_string(),
                        );
                    }

                    let params = map.remove("params");
                    if let Some(id_value) = id_value {
                        let Some(id) = parse_id_owned(id_value) else {
                            let _ = send_batch_or_direct_error_raw_id(
                                ctx.responder,
                                batch,
                                Value::Null,
                                INVALID_REQUEST,
                                "invalid request id",
                                None,
                            )
                            .await;
                            return Err("peer sent request with invalid id".to_string());
                        };

                        let request = IncomingRequest {
                            id,
                            method,
                            params,
                            responder: match batch {
                                Some(batch) => RequestResponder::batch(batch.clone()),
                                None => RequestResponder::direct(ctx.responder.clone()),
                            },
                        };

                        match ctx.request_tx.try_send(request) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(request)) => {
                                drop(
                                    request
                                        .responder
                                        .respond_error(
                                            &request.id,
                                            -32000,
                                            "client overloaded",
                                            None,
                                        )
                                        .await,
                                );
                            }
                            Err(mpsc::error::TrySendError::Closed(request)) => {
                                drop(
                                    request
                                        .responder
                                        .respond_error(
                                            &request.id,
                                            -32601,
                                            "no request handler installed",
                                            None,
                                        )
                                        .await,
                                );
                            }
                        }
                        return Ok(());
                    }

                    match ctx.notify_tx.try_send(Notification { method, params }) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            close_notification_queue_overflow(
                                ctx,
                                "server notification queue is full; closing connection to avoid silent data loss".to_string(),
                                &ctx.stats.dropped_notifications_full,
                            )
                            .await;
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            close_notification_queue_overflow(
                                ctx,
                                "server notification handler is unavailable; closing connection to avoid silent data loss".to_string(),
                                &ctx.stats.dropped_notifications_closed,
                            )
                            .await;
                        }
                    }
                    return Ok(());
                }
                Some(_) => {
                    if let Some(id_value) = map.remove("id") {
                        let id_value = error_response_id_or_null(id_value);
                        let _ = send_batch_or_direct_error_raw_id(
                            ctx.responder,
                            batch,
                            id_value,
                            INVALID_REQUEST,
                            "invalid request method",
                            None,
                        )
                        .await;
                    }
                    return Err("peer sent request with invalid method".to_string());
                }
                None => {}
            }

            handle_response(ctx.pending, ctx.cancelled_request_ids, Value::Object(map))
                .map_err(|err| err.to_string())
        }
        _ => {
            let _ = send_batch_or_direct_error_raw_id(
                ctx.responder,
                batch,
                Value::Null,
                INVALID_REQUEST,
                "invalid message",
                None,
            )
            .await;
            Err("peer sent non-object JSON-RPC message".to_string())
        }
    }
}

async fn send_batch_or_direct_error_raw_id(
    responder: &ClientHandle,
    batch: Option<&BatchResponseWriter>,
    id: Value,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> Result<(), Error> {
    let message = message.into();
    match batch {
        Some(batch) => {
            batch
                .push_immediate_response(outbound_error_response_value(
                    &id,
                    code,
                    &message,
                    data.as_ref(),
                )?)
                .await
        }
        None => {
            responder
                .respond_error_raw_id(id, code, message, data)
                .await
        }
    }
}

pub(crate) async fn read_line_limited<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut buf = Vec::with_capacity(max_bytes.min(READ_LINE_INITIAL_CAP_BYTES));
    if read_line_limited_into(reader, max_bytes, &mut buf).await? {
        Ok(Some(buf))
    } else {
        Ok(None)
    }
}

pub(crate) async fn read_line_limited_into<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
    buf: &mut Vec<u8>,
) -> Result<bool, std::io::Error> {
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.len() > max_bytes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "jsonrpc message too large",
                ));
            }
            maybe_shrink_line_buffer(buf, max_bytes);
            return Ok(!buf.is_empty());
        }

        let newline_pos = available.iter().position(|b| *b == b'\n');
        let take = newline_pos.map_or(available.len(), |idx| idx.saturating_add(1));
        // Allow only delimiter slack above the payload limit:
        // - up to 1 byte while scanning (possible trailing '\r' before '\n')
        // - up to 2 bytes when this chunk includes '\n' (possible "\r\n")
        let delimiter_slack = if newline_pos.is_some() { 2 } else { 1 };
        if buf.len().saturating_add(take) > max_bytes.saturating_add(delimiter_slack) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "jsonrpc message too large",
            ));
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);

        if newline_pos.is_some() {
            break;
        }
    }

    if buf.ends_with(b"\n") {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
    }

    if buf.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "jsonrpc message too large",
        ));
    }

    maybe_shrink_line_buffer(buf, max_bytes);
    Ok(true)
}

pub(crate) fn is_ascii_whitespace_only(line: &[u8]) -> bool {
    line.is_empty()
        || (line.first().is_some_and(u8::is_ascii_whitespace)
            && line.iter().all(u8::is_ascii_whitespace))
}

fn maybe_shrink_line_buffer(buf: &mut Vec<u8>, max_bytes: usize) {
    let retain = REUSABLE_LINE_BUFFER_RETAIN_BYTES.min(max_bytes);
    if retain == 0 {
        return;
    }
    // After occasional large messages, release surplus capacity once smaller traffic resumes.
    if buf.capacity() > retain && buf.len() <= retain {
        buf.shrink_to(retain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn read_line_limited_accepts_payload_equal_to_limit_with_lf() {
        let input = b"hello\n";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let line = match read_line_limited(&mut reader, 5).await {
            Ok(Some(line)) => line,
            Ok(None) => panic!("line available"),
            Err(err) => panic!("read succeeds: {err}"),
        };

        assert_eq!(line, b"hello");
    }

    #[tokio::test]
    async fn read_line_limited_accepts_payload_equal_to_limit_with_crlf() {
        let input = b"hello\r\n";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let line = match read_line_limited(&mut reader, 5).await {
            Ok(Some(line)) => line,
            Ok(None) => panic!("line available"),
            Err(err) => panic!("read succeeds: {err}"),
        };

        assert_eq!(line, b"hello");
    }

    #[tokio::test]
    async fn read_line_limited_rejects_payload_over_limit() {
        let input = b"helloo\n";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let err = read_line_limited(&mut reader, 5)
            .await
            .expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_line_limited_rejects_payload_over_limit_without_newline() {
        let input = b"helloo";
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input.as_slice()));

        let err = read_line_limited(&mut reader, 5)
            .await
            .expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_line_limited_into_releases_large_buffer_after_small_line() {
        let large = vec![b'x'; REUSABLE_LINE_BUFFER_RETAIN_BYTES * 2];
        let mut input = large.clone();
        input.push(b'\n');
        input.extend_from_slice(b"ok\n");

        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(input));
        let mut line = Vec::new();

        assert!(
            read_line_limited_into(&mut reader, large.len(), &mut line)
                .await
                .expect("large line must parse")
        );
        let large_capacity = line.capacity();
        assert!(large_capacity >= large.len());

        assert!(
            read_line_limited_into(&mut reader, large.len(), &mut line)
                .await
                .expect("small line must parse")
        );
        assert_eq!(line, b"ok");
        assert!(line.capacity() <= REUSABLE_LINE_BUFFER_RETAIN_BYTES);
    }

    #[test]
    fn ascii_whitespace_only_fast_path_keeps_semantics() {
        assert!(is_ascii_whitespace_only(b""));
        assert!(is_ascii_whitespace_only(b" \t\r\n"));
        assert!(!is_ascii_whitespace_only(b"{\"jsonrpc\":\"2.0\"}"));
        assert!(!is_ascii_whitespace_only(b"\xE3\x80\x80"));
    }

    #[tokio::test]
    async fn nested_batch_item_returns_invalid_request_error_in_batch_array() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = crate::Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(br#"[[{"jsonrpc":"2.0","method":"demo"}]]"#)
            .await
            .expect("write nested batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush nested batch");

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["error"]["code"], -32600);
        assert_eq!(items[0]["error"]["message"], "nested batch is not allowed");
        assert_eq!(items[0]["id"], Value::Null);

        drop(client);
    }

    #[tokio::test]
    async fn top_level_batch_request_returns_single_array_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = crate::Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(
                br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","method":"note"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
            )
            .await
            .expect("write batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush batch");

        let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("first request timeout")
            .expect("first request");
        assert_eq!(first.method, "first");

        let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("second request timeout")
            .expect("second request");
        assert_eq!(second.method, "second");

        first
            .respond_ok(serde_json::json!({"handled":"first"}))
            .await
            .expect("respond first");
        second
            .respond_error(-32001, "second failed", None)
            .await
            .expect("respond second");

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 2);
        assert_eq!(items.iter().filter(|item| item["id"] == 1).count(), 1);
        assert_eq!(items.iter().filter(|item| item["id"] == 2).count(), 1);
        assert!(
            items
                .iter()
                .any(|item| item["id"] == 1 && item["result"]["handled"] == "first"),
            "{items:?}"
        );
        assert!(
            items.iter().any(|item| {
                item["id"] == 2
                    && item["error"]["code"] == -32001
                    && item["error"]["message"] == "second failed"
            }),
            "{items:?}"
        );

        drop(client);
    }

    #[tokio::test]
    async fn dropped_direct_request_returns_internal_error_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = crate::Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(br#"{"jsonrpc":"2.0","id":1,"method":"first"}"#)
            .await
            .expect("write request");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush request");

        let request = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("request timeout")
            .expect("request");
        assert_eq!(request.method, "first");
        drop(request);

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse response");
        assert_eq!(response["id"], 1);
        assert_eq!(response["error"]["code"], -32603);
        assert_eq!(
            response["error"]["message"],
            "request handler dropped request without responding"
        );

        drop(client);
    }

    #[tokio::test]
    async fn dropped_batch_request_emits_internal_error_and_preserves_remaining_batch_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = crate::Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(
                br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
            )
            .await
            .expect("write batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush batch");

        let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("first request timeout")
            .expect("first request");
        assert_eq!(first.method, "first");

        let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("second request timeout")
            .expect("second request");
        assert_eq!(second.method, "second");

        second
            .respond_ok(serde_json::json!({"handled":"second"}))
            .await
            .expect("respond second");
        drop(first);

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 2);
        assert!(
            items.iter().any(|item| {
                item["id"] == 1
                    && item["error"]["code"] == -32603
                    && item["error"]["message"]
                        == "request handler dropped request without responding"
            }),
            "{items:?}"
        );
        assert!(
            items
                .iter()
                .any(|item| item["id"] == 2 && item["result"]["handled"] == "second"),
            "{items:?}"
        );

        drop(client);
    }

    #[test]
    fn dropped_batch_request_without_runtime_still_flushes_remaining_batch_response() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, server_read, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = crate::Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");
            let mut requests = client.take_requests().expect("request receiver");

            server_write
                .write_all(
                    br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
                )
                .await
                .expect("write batch");
            server_write.write_all(b"\n").await.expect("write newline");
            server_write.flush().await.expect("flush batch");

            let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
                .await
                .expect("first request timeout")
                .expect("first request");
            let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
                .await
                .expect("second request timeout")
                .expect("second request");

            second
                .respond_ok(serde_json::json!({"handled":"second"}))
                .await
                .expect("respond second");

            (first, server_read, client)
        });
        drop(runtime);

        drop(request);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build read runtime");

        runtime.block_on(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();
            let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .expect("response timeout")
                .expect("read response")
                .expect("response line");
            let response: Value = serde_json::from_str(&response_line).expect("parse batch");
            let items = response.as_array().expect("batch response array");
            assert_eq!(items.len(), 2);
            assert!(
                items.iter().any(|item| {
                    item["id"] == 1
                        && item["error"]["code"] == -32603
                        && item["error"]["message"]
                            == "request handler dropped request without responding"
                }),
                "{items:?}"
            );
            assert!(
                items
                    .iter()
                    .any(|item| item["id"] == 2 && item["result"]["handled"] == "second"),
                "{items:?}"
            );

            drop(client);
        });
    }

    #[test]
    fn dropped_direct_request_without_runtime_still_emits_internal_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, server_read, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = crate::Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");
            let mut requests = client.take_requests().expect("request receiver");

            server_write
                .write_all(br#"{"jsonrpc":"2.0","id":1,"method":"first"}"#)
                .await
                .expect("write request");
            server_write.write_all(b"\n").await.expect("write newline");
            server_write.flush().await.expect("flush request");

            let request = tokio::time::timeout(Duration::from_secs(1), requests.recv())
                .await
                .expect("request timeout")
                .expect("request");

            (request, server_read, client)
        });
        drop(runtime);

        drop(request);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build read runtime");

        runtime.block_on(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();
            let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .expect("response timeout")
                .expect("read response")
                .expect("response line");
            let response: Value = serde_json::from_str(&response_line).expect("parse response");
            assert_eq!(response["id"], 1);
            assert_eq!(response["error"]["code"], -32603);
            assert_eq!(
                response["error"]["message"],
                "request handler dropped request without responding"
            );

            drop(client);
        });
    }

    #[tokio::test]
    async fn dropping_request_clone_does_not_release_batch_slot_early() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = crate::Client::connect_io(client_read, client_write)
            .await
            .expect("connect client");
        let mut requests = client.take_requests().expect("request receiver");
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(
                br#"[{"jsonrpc":"2.0","id":1,"method":"first"},{"jsonrpc":"2.0","id":2,"method":"second"}]"#,
            )
            .await
            .expect("write batch");
        server_write.write_all(b"\n").await.expect("write newline");
        server_write.flush().await.expect("flush batch");

        let first = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("first request timeout")
            .expect("first request");
        let first_clone = first.clone();
        let second = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .expect("second request timeout")
            .expect("second request");

        drop(first_clone);
        second
            .respond_ok(serde_json::json!({"handled":"second"}))
            .await
            .expect("respond second");
        first
            .respond_ok(serde_json::json!({"handled":"first"}))
            .await
            .expect("respond first");

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse batch response");
        let items = response.as_array().expect("batch response array");
        assert_eq!(items.len(), 2);

        drop(client);
    }

    #[tokio::test]
    async fn invalid_jsonrpc_frame_closes_client_after_invalid_request_response() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = crate::Client::connect_io(client_read, client_write)
            .await
            .unwrap();
        let handle = client.handle();
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        server_write
            .write_all(br#"{"jsonrpc":"1.0","id":7,"method":"demo/callback"}"#)
            .await
            .unwrap();
        server_write.write_all(b"\n").await.unwrap();
        server_write.flush().await.unwrap();

        let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response line");
        let response: Value = serde_json::from_str(&response_line).expect("parse response");
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["error"]["message"], "invalid jsonrpc version");

        tokio::time::timeout(Duration::from_secs(1), async {
            while !client.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let err = client
            .request("demo/ping", serde_json::json!({}))
            .await
            .expect_err("closed client should reject new requests");
        assert!(matches!(err, Error::Protocol(_)));
        assert!(
            handle
                .close_reason()
                .as_deref()
                .is_some_and(|reason: &str| reason.contains("invalid jsonrpc version"))
        );
    }

    #[tokio::test]
    async fn notification_queue_overflow_closes_client_and_tracks_stats() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (_server_read, mut server_write) = tokio::io::split(server_stream);

        let mut options = crate::SpawnOptions::default();
        options.limits.notifications_capacity = 1;
        let client = crate::Client::connect_io_with_options(client_read, client_write, options)
            .await
            .unwrap();
        let handle = client.handle();

        let note = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "demo/notify",
            "params": {},
        });
        let mut out = serde_json::to_string(&note).unwrap();
        out.push('\n');
        server_write.write_all(out.as_bytes()).await.unwrap();
        server_write.write_all(out.as_bytes()).await.unwrap();
        server_write.flush().await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if handle.is_closed() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(client.stats().dropped_notifications_full, 1);
        assert!(
            handle
                .close_reason()
                .unwrap_or_default()
                .contains("notification queue is full")
        );
    }
}
