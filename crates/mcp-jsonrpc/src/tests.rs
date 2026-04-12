use super::*;
use error_kit::{ErrorCategory, ErrorRetryAdvice};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize};
use structured_text_kit::StructuredText;

fn detached_runtime_test_guard() -> &'static std::sync::Mutex<()> {
    static DETACHED_RUNTIME_TEST_GUARD: std::sync::OnceLock<std::sync::Mutex<()>> =
        std::sync::OnceLock::new();
    DETACHED_RUNTIME_TEST_GUARD.get_or_init(|| std::sync::Mutex::new(()))
}

fn test_detached_spawner() -> crate::background_runtime::DetachedSpawner {
    crate::background_runtime::reset_detached_runtime_test_state();
    let spawner = crate::background_runtime::DetachedSpawner::new();
    spawner.reset_for_test();
    spawner
}

fn test_client_handle(write: impl tokio::io::AsyncWrite + Send + Unpin + 'static) -> ClientHandle {
    use std::collections::HashMap;

    ClientHandle {
        write: Arc::new(tokio::sync::Mutex::new(Box::new(write))),
        next_id: Arc::new(AtomicI64::new(1)),
        pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
        max_message_bytes: normalize_max_message_bytes(0),
        max_pending_requests: 8,
        cancelled_request_ids: Arc::new(std::sync::Mutex::new(CancelledRequestIdsState::default())),
        stats: Arc::new(ClientStatsInner::default()),
        diagnostics: None,
        closed: Arc::new(AtomicBool::new(false)),
        close_reason: Arc::new(std::sync::Mutex::new(None)),
        stdout_log_write_error: Arc::new(std::sync::OnceLock::new()),
        lifecycle: None,
    }
}

#[test]
fn streamable_http_options_default_to_system_proxy_support() {
    let options = StreamableHttpOptions::default();
    assert_eq!(options.proxy_mode, StreamableHttpProxyMode::UseSystem);
}

#[test]
fn streamable_http_proxy_mode_default_uses_system_proxy_support() {
    assert_eq!(
        StreamableHttpProxyMode::default(),
        StreamableHttpProxyMode::UseSystem
    );
}

struct AlwaysFailWrite;

impl tokio::io::AsyncWrite for AlwaysFailWrite {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let _ = self;
        std::task::Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "forced batch write failure",
        )))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let _ = self;
        std::task::Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "forced batch flush failure",
        )))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let _ = self;
        std::task::Poll::Ready(Ok(()))
    }
}

struct PendingWrite;

impl tokio::io::AsyncWrite for PendingWrite {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let _ = self;
        std::task::Poll::Pending
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let _ = self;
        std::task::Poll::Pending
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let _ = self;
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn close_in_background_once_aborts_reader_and_transport_tasks() {
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct BlockingRead {
        dropped: Arc<AtomicBool>,
    }

    impl tokio::io::AsyncRead for BlockingRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl Drop for BlockingRead {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    struct OnDrop(Arc<AtomicBool>);

    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    let reader_dropped = Arc::new(AtomicBool::new(false));
    let transport_dropped = Arc::new(AtomicBool::new(false));
    let client = Client::connect_io(
        BlockingRead {
            dropped: Arc::clone(&reader_dropped),
        },
        tokio::io::sink(),
    )
    .await
    .expect("connect client");

    let transport_drop_guard = OnDrop(Arc::clone(&transport_dropped));
    client.register_transport_task(tokio::spawn(async move {
        let _transport_drop_guard = transport_drop_guard;
        std::future::pending::<()>().await;
    }));

    client.close_in_background_once("test background close");
    assert!(
        client.is_closed(),
        "background close should mark client closed"
    );

    tokio::time::timeout(Duration::from_secs(1), async {
        while !(reader_dropped.load(Ordering::Relaxed) && transport_dropped.load(Ordering::Relaxed))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background close should abort reader and transport tasks");
}

#[tokio::test]
async fn client_handle_close_aborts_reader_and_transport_tasks() {
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct BlockingRead {
        dropped: Arc<AtomicBool>,
    }

    impl tokio::io::AsyncRead for BlockingRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl Drop for BlockingRead {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    struct OnDrop(Arc<AtomicBool>);

    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    let reader_dropped = Arc::new(AtomicBool::new(false));
    let transport_dropped = Arc::new(AtomicBool::new(false));
    let client = Client::connect_io(
        BlockingRead {
            dropped: Arc::clone(&reader_dropped),
        },
        tokio::io::sink(),
    )
    .await
    .expect("connect client");

    let transport_drop_guard = OnDrop(Arc::clone(&transport_dropped));
    client.register_transport_task(tokio::spawn(async move {
        let _transport_drop_guard = transport_drop_guard;
        std::future::pending::<()>().await;
    }));

    let handle = client.handle();
    handle.close("test handle close").await;
    assert!(
        handle.is_closed(),
        "handle close should mark the client closed"
    );

    tokio::time::timeout(Duration::from_secs(1), async {
        while !(reader_dropped.load(Ordering::Relaxed) && transport_dropped.load(Ordering::Relaxed))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("handle close should abort reader and transport tasks");
}

#[tokio::test]
async fn schedule_close_once_aborts_reader_and_transport_tasks() {
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct BlockingRead {
        dropped: Arc<AtomicBool>,
    }

    impl tokio::io::AsyncRead for BlockingRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl Drop for BlockingRead {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    struct OnDrop(Arc<AtomicBool>);

    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    let reader_dropped = Arc::new(AtomicBool::new(false));
    let transport_dropped = Arc::new(AtomicBool::new(false));
    let client = Client::connect_io(
        BlockingRead {
            dropped: Arc::clone(&reader_dropped),
        },
        tokio::io::sink(),
    )
    .await
    .expect("connect client");

    let transport_drop_guard = OnDrop(Arc::clone(&transport_dropped));
    client.register_transport_task(tokio::spawn(async move {
        let _transport_drop_guard = transport_drop_guard;
        std::future::pending::<()>().await;
    }));

    let handle = client.handle();
    handle.schedule_close_once("test scheduled close".to_string());

    tokio::time::timeout(Duration::from_secs(1), async {
        while !(reader_dropped.load(Ordering::Relaxed) && transport_dropped.load(Ordering::Relaxed))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timeout close should abort reader and transport tasks");
}

#[test]
fn schedule_close_once_fallback_does_not_block_forever_on_locked_writer() {
    let _guard = detached_runtime_test_guard()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::background_runtime::reset_detached_runtime_test_state();
    crate::background_runtime::force_detached_runtime_spawn_failures(0, 1);

    let handle = test_client_handle(tokio::io::sink());
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let barrier_for_thread = Arc::clone(&barrier);
    let write = Arc::clone(&handle.write);
    let lock_thread = std::thread::spawn(move || {
        let _write_guard = write.blocking_lock();
        barrier_for_thread.wait();
        std::thread::sleep(CLOSE_LOCK_ACQUIRE_TIMEOUT + Duration::from_millis(50));
    });

    barrier.wait();
    let start = std::time::Instant::now();
    handle.schedule_close_once("forced fallback close".to_string());
    let elapsed = start.elapsed();

    assert!(
        handle.is_closed(),
        "close should still mark the handle closed"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "fallback close should stop waiting after a bounded interval, got {elapsed:?}"
    );

    lock_thread.join().expect("join lock thread");
    crate::background_runtime::reset_detached_runtime_test_state();
}

mod line_limit_tests {
    use super::*;

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
}

#[cfg(test)]
mod incoming_value_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn reserved_batch_slot_is_released_when_transport_is_already_closed() {
        let handle = test_client_handle(tokio::io::sink());
        handle.close_with_reason("closed for test").await;

        let batch = BatchResponseWriter::new(handle.clone()).reserve_request_slot();
        let err = batch
            .push_reserved_response(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "ok": true },
            }))
            .await
            .expect_err("closed transport should reject reserved response");

        assert!(
            matches!(err, Error::Protocol(ref protocol) if protocol.kind == ProtocolErrorKind::Closed),
            "{err:?}"
        );
        let completion_state = batch.state.completion_state.load(Ordering::Acquire);
        assert_eq!(completion_state & BatchResponseWriter::PENDING_MASK, 0);
        assert_eq!(
            completion_state & BatchResponseWriter::FINISHED_BIT,
            0,
            "failed response should only release the reserved slot"
        );
    }

    #[tokio::test]
    async fn batch_finish_errors_are_not_swallowed() {
        use std::collections::HashMap;

        let pending: PendingRequests = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancelled_request_ids: CancelledRequestIds =
            Arc::new(std::sync::Mutex::new(CancelledRequestIdsState::default()));
        let stats = Arc::new(ClientStatsInner::default());
        let (notify_tx, _notify_rx) = tokio::sync::mpsc::channel(1);
        let (request_tx, _request_rx) = tokio::sync::mpsc::channel(1);
        let handle = test_client_handle(AlwaysFailWrite);

        let err = handle_incoming_value(
            Value::Array(vec![Value::Array(vec![serde_json::json!({
                "jsonrpc": "2.0",
                "method": "demo",
            })])]),
            &pending,
            &cancelled_request_ids,
            &stats,
            &notify_tx,
            &request_tx,
            &handle,
        )
        .await
        .expect_err("batch flush failure should propagate");

        assert!(err.contains("batch response flush failed"), "{err}");
        let close_reason = handle.close_reason().expect("close reason");
        assert!(
            close_reason.contains("client transport write failed"),
            "{close_reason}"
        );
    }

    #[tokio::test]
    async fn nested_batch_item_returns_invalid_request_error_in_batch_array() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = Client::connect_io(client_read, client_write)
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

        let mut client = Client::connect_io(client_read, client_write)
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

        let mut client = Client::connect_io(client_read, client_write)
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

        let mut client = Client::connect_io(client_read, client_write)
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
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, server_read, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = Client::connect_io(client_read, client_write)
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
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn dropped_direct_request_without_runtime_still_emits_internal_error() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, server_read, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = Client::connect_io(client_read, client_write)
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
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn dropped_direct_request_without_runtime_closes_client_when_detached_helpers_are_unavailable()
    {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();
        crate::background_runtime::force_detached_runtime_spawn_failures(4, 2);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build setup runtime");

        let (request, client) = runtime.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(2048);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let (_server_read, mut server_write) = tokio::io::split(server_stream);

            let mut client = Client::connect_io(client_read, client_write)
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

            (request, client)
        });
        drop(runtime);

        let handle = client.handle();
        drop(request);

        let close_reason = std::time::Instant::now() + Duration::from_secs(1);
        let close_reason = loop {
            if let Some(reason) = handle.close_reason() {
                break reason;
            }
            assert!(
                std::time::Instant::now() < close_reason,
                "close reason recorded"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            close_reason.contains("failed to schedule dropped request response"),
            "{close_reason}"
        );
        assert!(handle.is_closed(), "client should fail closed");

        drop(client);
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn batch_flush_without_runtime_times_out_and_closes_transport() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();

        let handle = test_client_handle(PendingWrite);
        let batch = BatchResponseWriter::new(handle.clone()).reserve_request_slot();
        batch
            .state
            .completion_state
            .fetch_or(BatchResponseWriter::FINISHED_BIT, Ordering::AcqRel);

        batch.push_reserved_response_without_runtime(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32603,
                "message": "request handler dropped request without responding",
            },
        }));

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let close_reason = loop {
            if let Some(reason) = handle.close_reason() {
                break reason;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "batch flush should record close reason after timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        assert!(
            close_reason.contains("batch response flush timed out after"),
            "{close_reason}"
        );
        assert!(
            handle.is_closed(),
            "client should fail closed after flush timeout"
        );

        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[tokio::test]
    async fn dropping_request_clone_does_not_release_batch_slot_early() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = Client::connect_io(client_read, client_write)
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
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn max_message_bytes_zero_falls_back_to_default() {
        assert_eq!(
            normalize_max_message_bytes(0),
            Limits::default().max_message_bytes
        );
        assert_eq!(normalize_max_message_bytes(4096), 4096);
    }

    #[tokio::test]
    async fn invalid_json_line_closes_client_and_drains_pending_requests() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = Client::connect_io(client_read, client_write).await.unwrap();
        let handle = client.handle();
        let request_task = tokio::spawn({
            let client = handle.clone();
            async move { client.request("demo/ping", serde_json::json!({})).await }
        });

        let mut lines = tokio::io::BufReader::new(server_read).lines();
        let request_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("request timeout")
            .expect("read request")
            .expect("request line");
        let request: Value = serde_json::from_str(&request_line).expect("parse request");
        assert_eq!(request["method"], "demo/ping");

        server_write.write_all(b"not-json\n").await.unwrap();
        server_write.flush().await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if client.stats().invalid_json_lines >= 1 && client.is_closed() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let err = request_task
            .await
            .unwrap()
            .expect_err("request should fail closed");
        assert!(matches!(err, Error::Protocol(_)));
        assert!(
            handle
                .close_reason()
                .as_deref()
                .is_some_and(|reason: &str| reason.contains("invalid JSON line"))
        );
    }

    #[tokio::test]
    async fn invalid_jsonrpc_frame_closes_client_after_invalid_request_response() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let client = Client::connect_io(client_read, client_write).await.unwrap();
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

    #[test]
    fn spawn_detached_runs_tasks_without_tokio_runtime() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_task = Arc::clone(&counter);
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        spawner
            .spawn("test detached runtime", async move {
                counter_for_task.fetch_add(1, AtomicOrdering::Relaxed);
                done_tx.send(()).unwrap();
            })
            .expect("detached runtime should accept queued task");

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("detached runtime should execute queued task");
        assert_eq!(counter.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn spawn_detached_reports_error_when_worker_and_fallback_thread_are_unavailable() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        crate::background_runtime::force_detached_runtime_spawn_failures(2, 1);

        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_task = Arc::clone(&counter);
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let err = spawner
            .spawn("test detached runtime double spawn failure", async move {
                counter_for_task.fetch_add(1, AtomicOrdering::Relaxed);
                done_tx
                    .send(())
                    .expect("signal failed fallback task completion");
            })
            .expect_err("double detached-runtime failure should be reported");

        assert!(
            err.to_string().contains("fallback runtime unavailable"),
            "{err}"
        );
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "failed detached-runtime scheduling must not silently run the task"
        );
        assert_eq!(counter.load(AtomicOrdering::Relaxed), 0);
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_reports_error_when_worker_and_fallback_runtime_build_are_unavailable() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        crate::background_runtime::force_detached_runtime_spawn_failures(2, 0);
        crate::background_runtime::force_fallback_runtime_build_failures(1);

        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let err = spawner
            .spawn("test detached runtime fallback build failure", async move {
                done_tx
                    .send(())
                    .expect("signal failed fallback runtime build task completion");
            })
            .expect_err("fallback runtime build failure should be reported");

        assert!(
            err.to_string().contains("fallback runtime unavailable"),
            "{err}"
        );
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "failed detached-runtime scheduling must not silently run the task"
        );
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_recovers_after_worker_panic() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        let (panic_done_tx, panic_done_rx) = std::sync::mpsc::channel();

        spawner
            .spawn("test detached runtime panic", async move {
                panic_done_tx.send(()).expect("signal panic task start");
                panic!("boom");
            })
            .expect("panic task should still schedule");
        panic_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("panic task should start");

        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_task = Arc::clone(&counter);
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        spawner
            .spawn("test detached runtime restart", async move {
                counter_for_task.fetch_add(1, AtomicOrdering::Relaxed);
                done_tx.send(()).expect("signal recovery task completion");
            })
            .expect("detached runtime should recover after panic");

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("detached runtime should recover after worker panic");
        assert_eq!(counter.load(AtomicOrdering::Relaxed), 1);
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_fallback_runtime_is_still_detached() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        crate::background_runtime::force_detached_runtime_spawn_failures(2, 0);

        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let join_handle = std::thread::spawn(move || {
            let result = spawner.spawn("test detached fallback runtime", async move {
                started_tx
                    .send(())
                    .expect("signal detached fallback task start");
                let _ = release_rx.await;
                done_tx
                    .send(())
                    .expect("signal detached fallback task completion");
            });
            returned_tx
                .send(result)
                .expect("signal detached fallback scheduling result");
        });

        let schedule_result = returned_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback scheduling should return without waiting for task completion");
        schedule_result.expect("fallback detached runtime should accept queued task");

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback detached runtime should start task");
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "task should remain blocked until release signal arrives"
        );

        release_tx
            .send(())
            .expect("release detached fallback runtime task");
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback detached runtime should finish after release");
        join_handle
            .join()
            .expect("fallback detached runtime scheduling thread should exit cleanly");
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_fallback_runtime_keeps_blocked_tasks_isolated() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        crate::background_runtime::force_detached_runtime_spawn_failures(4, 0);

        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
        let (first_done_tx, first_done_rx) = std::sync::mpsc::channel();
        let (second_done_tx, second_done_rx) = std::sync::mpsc::channel();

        spawner
            .spawn("test detached fallback blocked task", async move {
                first_started_tx
                    .send(())
                    .expect("signal detached fallback blocked task start");
                let _ = release_rx.await;
                first_done_tx
                    .send(())
                    .expect("signal detached fallback blocked task completion");
            })
            .expect("first task should fall back to a dedicated runtime");

        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first fallback task should start");

        spawner
            .spawn("test detached fallback second task", async move {
                second_done_tx
                    .send(())
                    .expect("signal second detached fallback task completion");
            })
            .expect("second task should get an independent fallback runtime");

        second_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second fallback task should not be serialized behind the first");

        release_tx
            .send(())
            .expect("release blocked detached fallback task");
        first_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocked detached fallback task should finish after release");
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_falls_back_when_shared_worker_runtime_init_fails() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        crate::background_runtime::force_shared_worker_runtime_build_failures(2);

        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let join_handle = std::thread::spawn(move || {
            let result = spawner.spawn("test detached runtime init failure fallback", async move {
                started_tx
                    .send(())
                    .expect("signal fallback task start after init failure");
                let _ = release_rx.await;
                done_tx
                    .send(())
                    .expect("signal fallback task completion after init failure");
            });
            returned_tx
                .send(result)
                .expect("signal detached scheduling result after init failure");
        });

        let schedule_result = returned_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback scheduling should return after worker init failure");
        schedule_result.expect("runtime init failure should fall back to dedicated runtime");

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback runtime should start task after worker init failure");
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "task should stay blocked until release signal arrives"
        );

        release_tx
            .send(())
            .expect("release fallback task after init failure");
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback runtime should finish after init failure");
        join_handle
            .join()
            .expect("fallback detached scheduling thread should exit cleanly");
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_falls_back_when_shared_worker_drops_task_before_start() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();
        crate::background_runtime::force_shared_worker_drop_before_start(1);

        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let join_handle = std::thread::spawn(move || {
            let result = spawner.spawn("test detached worker pre-start drop", async move {
                started_tx.send(()).expect("signal fallback task start");
                let _ = release_rx.await;
                done_tx.send(()).expect("signal fallback task completion");
            });
            returned_tx
                .send(result)
                .expect("signal detached scheduling result");
        });

        let schedule_result = returned_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback scheduling should return after worker loss");
        schedule_result.expect("worker pre-start drop should fall back to dedicated runtime");

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fallback runtime should still start the task");
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "task should stay blocked until release signal arrives"
        );

        if release_tx.send(()).is_ok() {
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("fallback runtime should finish after release");
        }
        join_handle
            .join()
            .expect("fallback detached scheduling thread should exit cleanly");
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn spawn_detached_shared_worker_does_not_serialize_blocked_tasks() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawner = test_detached_spawner();

        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
        let (second_done_tx, second_done_rx) = std::sync::mpsc::channel();

        spawner
            .spawn("test detached shared worker blocked task", async move {
                first_started_tx
                    .send(())
                    .expect("signal shared worker blocked task start");
                let _ = release_rx.await;
            })
            .expect("shared worker should accept first task");

        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first shared worker task should start");

        spawner
            .spawn("test detached shared worker second task", async move {
                second_done_tx
                    .send(())
                    .expect("signal second shared worker task completion");
            })
            .expect("shared worker should accept second task");

        second_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second task should not be blocked behind first task");
        release_tx
            .send(())
            .expect("release blocked shared worker task");
        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn detached_spawners_do_not_share_process_global_worker() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();
        let spawn_count_before = crate::background_runtime::shared_worker_spawn_count();

        let first = test_detached_spawner();
        let second = test_detached_spawner();
        let (first_tx, first_rx) = std::sync::mpsc::channel();
        let (second_tx, second_rx) = std::sync::mpsc::channel();

        first
            .spawn("first detached spawner", async move {
                first_tx.send(()).expect("signal first detached worker");
            })
            .expect("first spawner should start a worker");
        first_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first detached worker should run");

        second
            .spawn("second detached spawner", async move {
                second_tx.send(()).expect("signal second detached worker");
            })
            .expect("second spawner should start its own worker");
        second_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second detached worker should run");

        let spawned_workers = crate::background_runtime::shared_worker_spawn_count()
            .saturating_sub(spawn_count_before);
        assert!(
            spawned_workers >= 2,
            "distinct detached spawners should start at least two shared workers, got {spawned_workers}"
        );
    }

    fn detached_runtime_test_guard() -> &'static std::sync::Mutex<()> {
        super::detached_runtime_test_guard()
    }

    #[test]
    fn invalid_json_samples_keep_latest_lines_when_buffer_is_full() {
        let diagnostics = DiagnosticsState::new(&DiagnosticsOptions {
            invalid_json_sample_lines: 2,
            invalid_json_sample_max_bytes: 64,
        })
        .expect("diagnostics enabled");

        diagnostics.record_invalid_json_line(b"invalid-1");
        diagnostics.record_invalid_json_line(b"invalid-2");
        diagnostics.record_invalid_json_line(b"invalid-3");

        assert_eq!(
            diagnostics.invalid_json_samples(),
            vec!["invalid-2".to_string(), "invalid-3".to_string()]
        );
    }

    #[tokio::test]
    async fn notification_queue_overflow_closes_client_and_tracks_stats() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (_server_read, mut server_write) = tokio::io::split(server_stream);

        let mut options = SpawnOptions::default();
        options.limits.notifications_capacity = 1;
        let client = Client::connect_io_with_options(client_read, client_write, options)
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

#[cfg(test)]
#[cfg(unix)]
mod wait_timeout_tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct BlockingWrite {
        entered: Arc<AtomicBool>,
    }

    impl tokio::io::AsyncWrite for BlockingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }
    }

    #[tokio::test]
    async fn wait_with_timeout_kill_still_kills_when_close_stage_times_out() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg("exec sleep 10");

        let mut client = match Client::spawn_command(cmd).await {
            Ok(client) => client,
            Err(err) => panic!("spawn client: {err}"),
        };
        let entered = Arc::new(AtomicBool::new(false));
        {
            let mut write = client.handle.write.lock().await;
            *write = Box::new(BlockingWrite {
                entered: entered.clone(),
            });
        }

        let wait_result = client
            .wait_with_timeout(
                Duration::from_millis(20),
                WaitOnTimeout::Kill {
                    kill_timeout: Duration::from_secs(1),
                },
            )
            .await;
        let child_status = match wait_result {
            Ok(status) => status,
            Err(err) => panic!("wait should kill child even when close stage times out: {err}"),
        };
        let status = match child_status {
            Some(status) => status,
            None => panic!("child exit status"),
        };

        assert!(entered.load(Ordering::Relaxed));
        assert!(!status.success());
    }

    #[test]
    fn request_optional_with_timeout_returns_error_without_tokio_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let (client_stream, _server_stream) = tokio::io::duplex(1024);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let client = Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");

            let err = client
                .request_optional_with_timeout("demo/request", None, Duration::from_secs(1))
                .await
                .expect_err("missing time driver should fail");
            match err {
                Error::Protocol(protocol_err) => {
                    assert_eq!(protocol_err.kind, ProtocolErrorKind::Other);
                    assert!(protocol_err.message.contains("time driver"));
                }
                other => panic!("expected protocol error, got {other:?}"),
            }
        });
    }

    #[test]
    fn wait_with_timeout_returns_error_without_tokio_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let (client_stream, _server_stream) = tokio::io::duplex(1024);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let mut client = Client::connect_io(client_read, client_write)
                .await
                .expect("connect client");

            let err = client
                .wait_with_timeout(Duration::from_secs(1), WaitOnTimeout::ReturnError)
                .await
                .expect_err("missing time driver should fail");
            match err {
                Error::Protocol(protocol_err) => {
                    assert_eq!(protocol_err.kind, ProtocolErrorKind::Other);
                    assert!(protocol_err.message.contains("time driver"));
                }
                other => panic!("expected protocol error, got {other:?}"),
            }
        });
    }
}

#[cfg(test)]
mod background_close_tests {
    use super::*;
    use std::io;
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;
    use std::task::{Context, Poll};
    use tokio::io::AsyncBufReadExt;

    struct ObserveShutdownWrite {
        shutdown_called: Arc<AtomicBool>,
    }

    struct FailingWrite {
        message: &'static str,
    }

    impl tokio::io::AsyncWrite for ObserveShutdownWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.shutdown_called.store(true, Ordering::SeqCst);
            Poll::Ready(Ok(()))
        }
    }

    impl tokio::io::AsyncWrite for FailingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(io::Error::other(self.message)))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(io::Error::other(self.message)))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn make_test_handle(write: Box<dyn AsyncWrite + Send + Unpin>) -> ClientHandle {
        ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(write)),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            max_message_bytes: Limits::default().max_message_bytes,
            max_pending_requests: 1,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(Mutex::new(None)),
            stdout_log_write_error: Arc::new(OnceLock::new()),
            lifecycle: None,
        }
    }

    #[test]
    fn schedule_close_once_without_runtime_drains_pending_without_panic() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();
        crate::background_runtime::force_detached_runtime_spawn_failures(0, 1);

        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let handle = ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(
                Box::new(tokio::io::sink()) as Box<dyn AsyncWrite + Send + Unpin>
            )),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: pending.clone(),
            max_message_bytes: Limits::default().max_message_bytes,
            max_pending_requests: 1,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(Mutex::new(None)),
            stdout_log_write_error: Arc::new(OnceLock::new()),
            lifecycle: None,
        };

        handle.schedule_close_once("closed outside runtime".to_string());

        assert!(handle.is_closed());
        assert_eq!(
            handle.close_reason().as_deref(),
            Some("closed outside runtime")
        );
        assert!(lock_pending(&pending).is_empty());

        let drained = rx
            .blocking_recv()
            .expect("pending request should be drained");
        let err = drained.expect_err("drained pending request must receive closed error");
        assert!(matches!(
            err,
            Error::Protocol(ProtocolError {
                kind: ProtocolErrorKind::Closed,
                ..
            })
        ));

        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[test]
    fn schedule_close_once_without_runtime_times_out_on_busy_writer_lock() {
        let _guard = detached_runtime_test_guard()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::background_runtime::reset_detached_runtime_test_state();
        crate::background_runtime::force_detached_runtime_spawn_failures(0, 1);

        let shutdown_called = Arc::new(AtomicBool::new(false));
        let handle = ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(Box::new(ObserveShutdownWrite {
                shutdown_called: Arc::clone(&shutdown_called),
            })
                as Box<dyn AsyncWrite + Send + Unpin>)),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            max_message_bytes: Limits::default().max_message_bytes,
            max_pending_requests: 1,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(Mutex::new(None)),
            stdout_log_write_error: Arc::new(OnceLock::new()),
            lifecycle: None,
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let write_guard = runtime.block_on(handle.write.lock());
        drop(runtime);

        let start = std::time::Instant::now();
        handle.schedule_close_once("close without runtime".to_string());
        let elapsed = start.elapsed();

        assert!(handle.is_closed());
        assert_eq!(
            handle.close_reason().as_deref(),
            Some("close without runtime")
        );
        assert!(
            !shutdown_called.load(Ordering::SeqCst),
            "fallback close should give up after bounded wait while writer lock is held"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "fallback close should stop waiting after a bounded interval, got {elapsed:?}"
        );

        drop(write_guard);
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !shutdown_called.load(Ordering::SeqCst),
            "timed-out close should not retry once the lock is released"
        );

        crate::background_runtime::reset_detached_runtime_test_state();
    }

    #[tokio::test]
    async fn write_json_line_io_error_closes_client() {
        let handle = make_test_handle(Box::new(FailingWrite {
            message: "simulated write failure",
        }));

        let err = handle
            .write_json_line(&serde_json::json!({"jsonrpc": "2.0"}))
            .await
            .expect_err("write should fail");
        assert!(matches!(err, Error::Io(_)));
        assert!(handle.is_closed(), "write failure should close the client");
        let close_reason = handle.close_reason().expect("close reason");
        assert!(close_reason.contains("client transport write failed"));
        assert!(close_reason.contains("simulated write failure"));
    }

    #[tokio::test]
    async fn dropped_direct_request_response_write_failure_closes_client() {
        let handle = make_test_handle(Box::new(FailingWrite {
            message: "simulated dropped-response write failure",
        }));

        drop(IncomingRequest {
            id: Id::Integer(1),
            method: "demo/test".to_string(),
            params: None,
            responder: RequestResponder::direct(handle.clone()),
            owner_count: Arc::new(AtomicUsize::new(1)),
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handle.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropped request write failure should close client");

        let close_reason = handle.close_reason().expect("close reason");
        assert!(
            close_reason.contains("simulated dropped-response write failure"),
            "{close_reason}"
        );
    }

    #[tokio::test]
    async fn only_last_dropped_request_clone_emits_internal_error_response() {
        let (client_stream, server_stream) = tokio::io::duplex(2048);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let mut client = Client::connect_io(client_read, client_write)
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
        let clone = request.clone();

        drop(clone);
        let no_response = tokio::time::timeout(Duration::from_millis(20), lines.next_line()).await;
        assert!(
            no_response.is_err(),
            "non-final clone drop should not emit a response, got {no_response:?}"
        );

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn batch_finish_and_last_reserved_response_flush_once_when_started_together() {
        for _ in 0..256 {
            let (write, read) = tokio::io::duplex(1024);
            let handle = ClientHandle {
                write: Arc::new(tokio::sync::Mutex::new(
                    Box::new(write) as Box<dyn AsyncWrite + Send + Unpin>
                )),
                next_id: Arc::new(AtomicI64::new(1)),
                pending: Arc::new(Mutex::new(HashMap::new())),
                max_message_bytes: Limits::default().max_message_bytes,
                max_pending_requests: 1,
                cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
                stats: Arc::new(ClientStatsInner::default()),
                diagnostics: None,
                closed: Arc::new(AtomicBool::new(false)),
                close_reason: Arc::new(Mutex::new(None)),
                stdout_log_write_error: Arc::new(OnceLock::new()),
                lifecycle: None,
            };
            let batch = BatchResponseWriter::new(handle);
            let reserved = batch.reserve_request_slot();
            let start = Arc::new(tokio::sync::Barrier::new(3));

            let finish_task = {
                let batch = batch.clone();
                let start = Arc::clone(&start);
                tokio::spawn(async move {
                    start.wait().await;
                    batch.finish().await.expect("finish batch");
                })
            };
            let respond_task = {
                let start = Arc::clone(&start);
                tokio::spawn(async move {
                    start.wait().await;
                    reserved
                        .push_reserved_response(serde_json::json!({"id": 1, "result": "ok"}))
                        .await
                        .expect("push reserved response");
                })
            };

            start.wait().await;
            finish_task.await.expect("finish join");
            respond_task.await.expect("respond join");

            let mut lines = tokio::io::BufReader::new(read).lines();
            let response_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .expect("response timeout")
                .expect("read response")
                .expect("response line");
            let response: Value = serde_json::from_str(&response_line).expect("parse response");
            assert_eq!(response, serde_json::json!([{"id": 1, "result": "ok"}]));
            assert!(
                tokio::time::timeout(Duration::from_millis(50), lines.next_line())
                    .await
                    .is_err(),
                "batch should flush exactly once"
            );
        }
    }

    #[tokio::test]
    async fn outbound_notify_request_and_response_respect_max_message_bytes() {
        let handle = ClientHandle {
            write: Arc::new(tokio::sync::Mutex::new(
                Box::new(tokio::io::sink()) as Box<dyn AsyncWrite + Send + Unpin>
            )),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            max_message_bytes: 64,
            max_pending_requests: 4,
            cancelled_request_ids: Arc::new(Mutex::new(CancelledRequestIdsState::default())),
            stats: Arc::new(ClientStatsInner::default()),
            diagnostics: None,
            closed: Arc::new(AtomicBool::new(false)),
            close_reason: Arc::new(Mutex::new(None)),
            stdout_log_write_error: Arc::new(OnceLock::new()),
            lifecycle: None,
        };

        let oversized = serde_json::json!({
            "payload": "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz"
        });

        let notify_err = handle
            .notify("demo/notify", Some(oversized.clone()))
            .await
            .expect_err("oversized notify should fail before write");
        assert!(
            notify_err
                .to_string()
                .contains("outbound jsonrpc message too large")
        );

        let request_err = handle
            .request("demo/request", oversized.clone())
            .await
            .expect_err("oversized request should fail before write");
        assert!(
            request_err
                .to_string()
                .contains("outbound jsonrpc message too large")
        );
        assert!(lock_pending(&handle.pending).is_empty());

        let response_err = handle
            .respond_ok(Id::Integer(1), oversized)
            .await
            .expect_err("oversized response should fail before write");
        assert!(
            response_err
                .to_string()
                .contains("outbound jsonrpc message too large")
        );
    }
}

#[cfg(test)]
mod cancelled_request_ids_tests {
    use super::*;

    #[test]
    fn cancelled_request_id_eviction_preserves_latest_reinserted_id() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Integer(1);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        assert!(take_cancelled_request_id(&cancelled_request_ids, &id));

        // Reinsert the same id, then push enough unique ids to evict stale queue entries.
        remember_cancelled_request_id(&cancelled_request_ids, &id);
        for value in 2..=(CANCELLED_REQUEST_IDS_MAX as i64) {
            remember_cancelled_request_id(&cancelled_request_ids, &Id::Integer(value));
        }

        assert!(take_cancelled_request_id(&cancelled_request_ids, &id));
        assert!(!take_cancelled_request_id(&cancelled_request_ids, &id));

        let guard = lock_cancelled_request_ids(&cancelled_request_ids);
        assert!(guard.order.len() <= CANCELLED_REQUEST_IDS_MAX);
    }

    #[test]
    fn cancelled_request_id_type_mismatch_consumes_counterpart_entry() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Integer(7);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        assert!(take_cancelled_request_id_type_mismatch(
            &cancelled_request_ids,
            &Id::String("7".to_string())
        ));
        assert!(!take_cancelled_request_id(&cancelled_request_ids, &id));
    }

    #[test]
    fn cancelled_request_id_type_mismatch_consumes_large_unsigned_counterpart_entry() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Unsigned(u64::MAX);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        assert!(take_cancelled_request_id_type_mismatch(
            &cancelled_request_ids,
            &Id::String(u64::MAX.to_string())
        ));
        assert!(!take_cancelled_request_id(&cancelled_request_ids, &id));
    }

    #[test]
    fn cancelled_request_id_duplicate_insert_refreshes_recency() {
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));
        let id = Id::Integer(1);

        remember_cancelled_request_id(&cancelled_request_ids, &id);
        for value in 2..=(CANCELLED_REQUEST_IDS_MAX as i64) {
            remember_cancelled_request_id(&cancelled_request_ids, &Id::Integer(value));
        }
        // Refresh the same id after the queue is full so its "latest generation" becomes recent.
        remember_cancelled_request_id(&cancelled_request_ids, &id);
        remember_cancelled_request_id(
            &cancelled_request_ids,
            &Id::Integer(CANCELLED_REQUEST_IDS_MAX as i64 + 1),
        );

        assert!(take_cancelled_request_id(&cancelled_request_ids, &id));
    }
}

#[cfg(test)]
mod response_routing_tests {
    use super::*;

    #[test]
    fn handle_response_routes_rpc_error_with_data() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));

        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32000,
                "message": "boom",
                "data": { "k": "v" }
            }
        });

        handle_response(&pending, &cancelled_request_ids, response).expect("handle response");

        let err = rx
            .blocking_recv()
            .expect("pending response channel")
            .expect_err("rpc error expected");
        match err {
            Error::Rpc {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32000);
                assert_eq!(message, "boom");
                assert_eq!(data, Some(serde_json::json!({ "k": "v" })));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn handle_response_rejects_result_and_error_together() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));

        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Integer(1), tx);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "ok": true },
            "error": {
                "code": -32000,
                "message": "boom"
            }
        });

        handle_response(&pending, &cancelled_request_ids, response).expect("handle response");

        let err = rx
            .blocking_recv()
            .expect("pending response channel")
            .expect_err("protocol error expected");
        match err {
            Error::Protocol(protocol_err) => {
                assert_eq!(protocol_err.kind, ProtocolErrorKind::InvalidMessage);
                assert!(
                    protocol_err
                        .message
                        .contains("must include exactly one of result/error")
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn handle_response_routes_large_unsigned_numeric_id() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_request_ids = Arc::new(Mutex::new(CancelledRequestIdsState::default()));

        let (tx, rx) = oneshot::channel();
        lock_pending(&pending).insert(Id::Unsigned(u64::MAX), tx);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": u64::MAX,
            "result": { "ok": true }
        });

        handle_response(&pending, &cancelled_request_ids, response).expect("handle response");

        let result = rx
            .blocking_recv()
            .expect("pending response channel")
            .expect("result payload expected");
        assert_eq!(result, serde_json::json!({ "ok": true }));
        assert!(lock_pending(&pending).is_empty());
    }
}

#[cfg(test)]
mod error_record_tests {
    use super::*;

    #[test]
    fn protocol_wait_timeout_maps_to_retryable_timeout_record() {
        let err = Error::protocol(ProtocolErrorKind::WaitTimeout, "wait timed out after 1s");

        let record = err.error_record();

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.protocol.wait_timeout");
        assert_eq!(record.category(), ErrorCategory::Timeout);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::Retryable);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("json-rpc wait timed out")
        );
        assert_eq!(
            record
                .diagnostic_text()
                .and_then(StructuredText::freeform_text),
            Some("wait timed out after 1s")
        );
    }

    #[test]
    fn rpc_method_not_found_maps_to_not_found_record() {
        let err = Error::Rpc {
            code: -32601,
            message: String::from("tools/list"),
            data: None,
        };

        let record = err.error_record();

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.rpc.method_not_found");
        assert_eq!(record.category(), ErrorCategory::NotFound);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("remote json-rpc method not found")
        );
        assert_eq!(
            record
                .diagnostic_text()
                .and_then(StructuredText::freeform_text),
            Some("remote json-rpc error -32601: tools/list")
        );
    }

    #[test]
    fn into_error_record_preserves_io_source() {
        let err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ));

        let record = err.into_error_record();

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.io.permission_denied");
        assert_eq!(record.category(), ErrorCategory::PermissionDenied);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record.user_text().freeform_text(),
            Some("json-rpc transport permission denied")
        );
        assert_eq!(
            record
                .source_ref()
                .expect("io source should be preserved")
                .to_string(),
            "permission denied"
        );
    }

    #[test]
    fn public_protocol_error_types_still_map_into_error_record() {
        let err = Error::Protocol(ProtocolError::new(
            ProtocolErrorKind::InvalidInput,
            "headers contain reserved mcp-session-id",
        ));

        let record = error_kit::ErrorRecord::from(err);

        assert_eq!(record.code().as_str(), "mcp_jsonrpc.protocol.invalid_input");
        assert_eq!(record.category(), ErrorCategory::InvalidInput);
        assert_eq!(record.retry_advice(), ErrorRetryAdvice::DoNotRetry);
        assert_eq!(
            record
                .diagnostic_text()
                .and_then(StructuredText::freeform_text),
            Some("headers contain reserved mcp-session-id")
        );
    }
}
