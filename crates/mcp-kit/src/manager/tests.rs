use super::*;
use crate::Transport;
#[cfg(not(windows))]
use crate::test_support::{CurrentDirRestoreGuard, cwd_test_guard};
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

fn seed_manager_side_state(manager: &mut Manager, server_name: &str) {
    manager
        .protocol_version_mismatches
        .push(ProtocolVersionMismatch {
            server_name: ServerName::parse(server_name).unwrap(),
            client_protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            server_protocol_version: "1900-01-01".to_string(),
        });
    manager
        .server_handler_timeout_counts
        .counter_for(&ServerName::parse(server_name).unwrap())
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    "<non-string panic payload>".to_string()
}

#[cfg(unix)]
#[test]
fn stable_connection_cwd_identity_preserves_symlink_parent_semantics() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let real = tempdir.path().join("real");
    let base = tempdir.path().join("base");
    let child = real.join("child");
    let sibling = real.join("sibling");
    std::fs::create_dir_all(&child).expect("create child");
    std::fs::create_dir_all(&sibling).expect("create sibling");
    std::fs::create_dir_all(&base).expect("create base");
    std::os::unix::fs::symlink(&child, base.join("link")).expect("create symlink");

    let resolved =
        super::path_identity::stable_connection_cwd_identity(&base.join("link/../sibling"))
            .expect("resolve symlink parent semantics");
    assert_eq!(resolved, sibling);
}

#[cfg(unix)]
#[test]
fn stable_connection_cwd_identity_keeps_symlink_target_for_missing_suffix() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let real = tempdir.path().join("real");
    let base = tempdir.path().join("base");
    let child = real.join("child");
    std::fs::create_dir_all(&child).expect("create child");
    std::fs::create_dir_all(&base).expect("create base");
    std::os::unix::fs::symlink(&child, base.join("link")).expect("create symlink");

    let resolved =
        super::path_identity::stable_connection_cwd_identity(&base.join("link/../missing/nested"))
            .expect("resolve missing suffix through symlink target");
    assert_eq!(resolved, real.join("missing/nested"));
}

#[test]
fn stable_connection_cwd_identity_collapses_missing_parent_segments_lexically() {
    let root = std::env::temp_dir().join("mcp-kit-missing-parent");
    let resolved =
        super::path_identity::stable_connection_cwd_identity(&root.join("missing/../child"))
            .expect("resolve missing parent lexically");
    let expected = super::path_identity::stable_connection_cwd_identity(&root.join("child"))
        .expect("resolve expected child");
    assert_eq!(resolved, expected);
}

#[test]
fn stable_connection_cwd_identity_keeps_absolute_paths_absolute_after_parent_segments() {
    let mut path = std::env::temp_dir();
    path.push("..");
    path.push("..");
    path.push("mcp-kit-absolute-child");

    let resolved = super::path_identity::stable_connection_cwd_identity(&path)
        .expect("resolve absolute path with parent segments");
    assert!(resolved.is_absolute(), "resolved path must stay absolute");
}

#[test]
fn roots_capability_is_inserted() {
    let mut capabilities = serde_json::json!({});
    ensure_roots_capability(&mut capabilities);
    assert!(capabilities.get("roots").is_some());
    assert!(capabilities.get("roots").unwrap().is_object());
}

#[test]
fn roots_capability_overwrites_non_object() {
    let mut capabilities = serde_json::json!({ "roots": true });
    ensure_roots_capability(&mut capabilities);
    assert!(capabilities.get("roots").unwrap().is_object());
}

#[test]
fn roots_capability_normalizes_non_object_container() {
    let mut capabilities = serde_json::json!(true);
    ensure_roots_capability(&mut capabilities);
    assert_eq!(capabilities, serde_json::json!({ "roots": {} }));
}

#[test]
fn with_protocol_version_rejects_empty_input() {
    let err = match Manager::default().with_protocol_version("   ") {
        Ok(_) => panic!("empty protocol version should be rejected"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), crate::ErrorKind::Config);
    assert!(
        err.to_string()
            .contains("protocol version must not be empty")
    );
}

#[test]
fn with_capabilities_rejects_non_object() {
    let err = match Manager::default().with_capabilities(serde_json::json!(["not", "an", "object"]))
    {
        Ok(_) => panic!("non-object capabilities should be rejected"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), crate::ErrorKind::Config);
    assert!(
        err.to_string()
            .contains("capabilities must be a JSON object")
    );
}

#[test]
fn with_capabilities_normalizes_non_object_roots_entry() {
    let manager = Manager::default()
        .with_capabilities(serde_json::json!({ "roots": true }))
        .expect("object-shaped capabilities should be accepted")
        .with_roots(vec![Root {
            uri: "file:///tmp".to_string(),
            name: Some("tmp".to_string()),
        }]);

    assert!(manager.capabilities.get("roots").is_some());
    assert!(
        manager
            .capabilities
            .get("roots")
            .expect("roots capability")
            .is_object()
    );
}

#[tokio::test]
async fn get_or_connect_unknown_server_is_config_error_kind() {
    let cfg = Config::new(
        crate::ClientConfig::default(),
        std::collections::BTreeMap::new(),
    );
    let mut manager = Manager::default();

    let err = manager
        .get_or_connect(&cfg, "missing", absolute_test_cwd())
        .await
        .unwrap_err();

    assert_eq!(err.kind(), crate::ErrorKind::Config);
    assert!(err.to_string().contains("unknown mcp server: missing"));
}

#[test]
fn prepare_transport_connect_accepts_whitespace_normalized_server_name() {
    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        ServerName::parse("srv").expect("server name"),
        ServerConfig::stdio(vec!["mcp-srv".to_string()]).expect("server config"),
    );
    let config = Config::new(crate::ClientConfig::default(), servers);
    let mut manager = Manager::default().with_trust_mode(TrustMode::Trusted);
    let cwd = absolute_test_cwd();

    let prepared = manager
        .prepare_transport_connect(&config, "  srv  ", cwd)
        .expect("prepare transport connect")
        .expect("prepared transport connect");

    assert_eq!(prepared.server_name_key.as_str(), "srv");
    assert_eq!(prepared.server_cfg.transport(), Transport::Stdio);
}

#[tokio::test]
async fn request_connected_missing_server_is_manager_state_error_kind() {
    let mut manager = Manager::default();

    let err = manager
        .request_connected("missing", "ping", None)
        .await
        .unwrap_err();

    assert_eq!(err.kind(), crate::ErrorKind::ManagerState);
    assert!(
        err.to_string()
            .contains("mcp server not connected: missing")
    );
}

#[test]
fn built_in_roots_list_requires_roots() {
    assert!(super::handlers::try_handle_built_in_request("roots/list", None).is_none());
}

#[test]
fn built_in_roots_list_returns_expected_shape() {
    let roots = Arc::new(vec![Root {
        uri: "file:///tmp".to_string(),
        name: Some("tmp".to_string()),
    }]);

    let result =
        super::handlers::try_handle_built_in_request("roots/list", Some(&roots)).expect("result");
    assert_eq!(
        result,
        serde_json::json!({
            "roots": [{ "uri": "file:///tmp", "name": "tmp" }]
        })
    );
}

#[test]
fn stdout_log_path_within_root_accepts_relative_path() {
    let root = std::env::temp_dir().join("workspace");
    assert!(
        stdout_log_path_within_root(Path::new("logs/server.stdout.log"), &root)
            .expect("stdout_log within root")
    );
}

#[test]
fn stdout_log_path_within_root_rejects_relative_parent_escape() {
    let root = std::env::temp_dir().join("workspace");
    assert!(
        !stdout_log_path_within_root(Path::new("../outside.log"), &root)
            .expect("stdout_log parent escape should be rejected")
    );
}

#[test]
fn stdout_log_path_within_root_accepts_absolute_path_after_root_absolutize() {
    let base = std::env::temp_dir();
    let root = absolutize_with_base(Path::new("workspace"), &base);
    let log_path = root.join("logs/server.stdout.log");
    assert!(stdout_log_path_within_root(&log_path, &root).expect("absolute path within root"));
}

#[cfg(unix)]
#[test]
fn stdout_log_path_within_root_rejects_symlink_escape() {
    let tempdir = tempfile::tempdir().unwrap();
    let root = tempdir.path().join("workspace");
    let outside = tempdir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("logs")).unwrap();

    assert!(
        !stdout_log_path_within_root(Path::new("logs/server.stdout.log"), &root)
            .expect("symlink escape should be rejected")
    );
}

#[cfg(unix)]
#[test]
fn stdout_log_path_within_root_rejects_symlink_parent_escape() {
    let tempdir = tempfile::tempdir().unwrap();
    let root = tempdir.path().join("workspace");
    let outside = tempdir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

    assert!(
        !stdout_log_path_within_root(Path::new("escape/../logs/server.stdout.log"), &root)
            .expect("symlink parent escape should be rejected")
    );
}

#[cfg(unix)]
#[test]
fn stdout_log_path_within_root_accepts_symlink_that_stays_within_root() {
    let tempdir = tempfile::tempdir().unwrap();
    let root = tempdir.path().join("workspace");
    let real_logs = root.join("real-logs");
    std::fs::create_dir_all(&real_logs).unwrap();
    std::os::unix::fs::symlink(&real_logs, root.join("logs")).unwrap();

    assert!(
        stdout_log_path_within_root(Path::new("logs/server.stdout.log"), &root)
            .expect("symlink staying within root should be accepted")
    );
}

#[test]
fn connection_wait_with_timeout_returns_error_without_tokio_time_driver() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        let (client_stream, _server_stream) = tokio::io::duplex(64);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
            .await
            .expect("client connect");
        let connection = Connection {
            id: next_connection_id(),
            child: None,
            client,
            handler_tasks: Vec::new(),
        };

        let err = connection
            .wait_with_timeout(
                Duration::from_secs(1),
                mcp_jsonrpc::WaitOnTimeout::ReturnError,
            )
            .await
            .expect_err("missing time driver should fail");
        assert!(err.to_string().contains("time driver"));
    });
}

#[test]
fn session_notify_returns_error_without_tokio_time_driver() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        let (client_stream, _server_stream) = tokio::io::duplex(64);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
            .await
            .expect("client connect");
        let connection = Connection {
            id: next_connection_id(),
            child: None,
            client,
            handler_tasks: Vec::new(),
        };
        let session = Session::new(
            ServerName::parse("demo").expect("server name"),
            connection,
            serde_json::json!({}),
            Duration::from_secs(1),
        );

        let err = session
            .notify("demo/notify", None)
            .await
            .expect_err("missing time driver should fail");
        assert!(err.to_string().contains("time driver"));
    });
}

#[test]
fn stdout_log_path_within_root_rejects_outside_absolute_path() {
    let root = std::env::temp_dir().join("workspace");
    let log_path = std::env::temp_dir().join("other/server.stdout.log");
    assert!(!stdout_log_path_within_root(&log_path, &root).expect("outside absolute path"));
}

#[test]
fn stdout_log_path_within_root_accepts_equivalent_root_with_parent_segments() {
    let root = std::env::temp_dir().join("workspace");
    let root_with_parent = root.join("nested").join("..");
    let log_path = root.join("logs/server.stdout.log");
    assert!(
        stdout_log_path_within_root(&log_path, &root_with_parent)
            .expect("equivalent root with parent segments")
    );
}

#[cfg(unix)]
#[test]
fn stdout_log_path_within_root_propagates_non_not_found_errors() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path().join("workspace");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("logs"), b"not a directory").expect("create blocking file");

    let err = stdout_log_path_within_root(Path::new("logs/server.stdout.log"), &root)
        .expect_err("non-directory existing prefix should be reported");
    assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
}

#[test]
fn resolve_config_connection_cwd_rejects_relative_cwd_without_thread_root() {
    let err = resolve_config_connection_cwd(None, Path::new("relative"))
        .expect_err("relative cwd should require a config thread root");
    assert!(
        err.to_string()
            .contains("relative MCP cwd requires a loaded config path/thread root"),
        "{err:#}"
    );
}

#[test]
fn resolve_connection_cwd_rejects_relative_cwd_without_explicit_base() {
    let err = resolve_connection_cwd(Path::new("relative"))
        .expect_err("relative cwd should require an explicit absolute base");
    assert!(
        err.to_string()
            .contains("relative MCP cwd requires an explicit absolute base"),
        "{err:#}"
    );
}

#[test]
fn resolve_connection_cwd_with_base_rejects_relative_base() {
    let err = resolve_connection_cwd_with_base(Some(Path::new("relative-base")), Path::new("cwd"))
        .expect_err("relative base should be rejected");
    assert!(
        err.to_string()
            .contains("relative MCP cwd base must be absolute"),
        "{err:#}"
    );
}

#[test]
fn resolve_config_connection_cwd_rejects_relative_escape_outside_thread_root() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let thread_root = tempdir.path().join("thread-root");
    std::fs::create_dir_all(&thread_root).expect("create thread root");
    let err = resolve_config_connection_cwd(Some(&thread_root), Path::new("../outside"))
        .expect_err("relative cwd escape should be rejected");
    assert!(
        err.to_string()
            .contains("relative MCP cwd must stay within root"),
        "{err:#}"
    );
}

#[cfg(unix)]
#[test]
fn resolve_config_connection_cwd_rejects_symlink_escape_outside_thread_root() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path().join("workspace");
    let outside = tempdir.path().join("outside");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::create_dir_all(&outside).expect("create outside");
    std::os::unix::fs::symlink(&outside, root.join("escape-link")).expect("create symlink");

    let err = resolve_config_connection_cwd(Some(&root), Path::new("escape-link/child"))
        .expect_err("symlink escape should be rejected");
    assert!(
        err.to_string()
            .contains("relative MCP cwd must stay within root"),
        "{err:#}"
    );
}

#[cfg(unix)]
#[test]
fn resolve_connection_cwd_with_base_propagates_non_not_found_errors() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let base = tempdir.path().join("workspace");
    std::fs::create_dir_all(&base).expect("create base");
    std::fs::write(base.join("file"), b"not a directory").expect("create blocking file");

    let err = resolve_connection_cwd_with_base(Some(&base), Path::new("file/child"))
        .expect_err("non-directory existing prefix should be reported");
    let io_err = err.downcast_ref::<std::io::Error>().expect("io error");
    assert_eq!(io_err.kind(), std::io::ErrorKind::NotADirectory);
}

#[test]
fn resolve_config_connection_cwd_rejects_relative_thread_root() {
    let err = resolve_config_connection_cwd(Some(Path::new("relative-root")), Path::new("cwd"))
        .expect_err("relative config root should be rejected");
    assert!(
        err.to_string()
            .contains("relative MCP cwd base must be absolute"),
        "{err:#}"
    );
}

#[test]
fn try_from_config_rejects_invalid_client_config() {
    let config = Config::new(
        crate::ClientConfig {
            capabilities: Some(serde_json::json!(1)),
            ..Default::default()
        },
        std::collections::BTreeMap::new(),
    );
    let err =
        match Manager::try_from_config(&config, "test-client", "0.0.0", Duration::from_secs(1)) {
            Ok(_) => panic!("expected error"),
            Err(err) => err,
        };
    assert!(err.to_string().contains("capabilities"), "err={err:#}");
}

#[test]
fn from_config_panics_on_invalid_client_config() {
    let config = Config::new(
        crate::ClientConfig {
            capabilities: Some(serde_json::json!(1)),
            ..Default::default()
        },
        std::collections::BTreeMap::new(),
    );
    let panic = std::panic::catch_unwind(|| {
        let _ = Manager::from_config(&config, "test-client", "0.0.0", Duration::from_secs(1));
    })
    .expect_err("invalid client config should panic");
    let message = panic_message(&panic);
    assert!(
        message.contains("validated Config"),
        "panic should explain the contract: {message}"
    );
    assert!(message.contains("capabilities"), "panic={message}");
}

#[test]
fn try_from_config_rejects_invalid_server_config() {
    let mut server = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
    server
        .env_http_headers_mut()
        .unwrap()
        .insert("MCP-Protocol-Version".to_string(), "MCP_TOKEN".to_string());

    let mut servers = std::collections::BTreeMap::new();
    servers.insert(ServerName::parse("srv").unwrap(), server);
    let config = Config::new(crate::ClientConfig::default(), servers);

    let err =
        match Manager::try_from_config(&config, "test-client", "0.0.0", Duration::from_secs(1)) {
            Ok(_) => panic!("expected error"),
            Err(err) => err,
        };
    let msg = err.to_string();
    assert!(msg.contains("server=srv"), "err={err:#}");
    assert!(msg.contains("invalid mcp server config"), "err={err:#}");
    assert!(msg.contains("reserved by transport"), "err={err:#}");
}

#[test]
fn from_config_panics_on_invalid_server_config() {
    let mut server = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
    server
        .env_http_headers_mut()
        .unwrap()
        .insert("MCP-Protocol-Version".to_string(), "MCP_TOKEN".to_string());

    let mut servers = std::collections::BTreeMap::new();
    servers.insert(ServerName::parse("srv").unwrap(), server);
    let config = Config::new(crate::ClientConfig::default(), servers);

    let panic = std::panic::catch_unwind(|| {
        let _ = Manager::from_config(&config, "test-client", "0.0.0", Duration::from_secs(1));
    })
    .expect_err("invalid server config should panic");
    let message = panic_message(&panic);
    assert!(
        message.contains("validated Config"),
        "panic should explain the contract: {message}"
    );
    assert!(message.contains("server=srv"), "panic={message}");
    assert!(message.contains("reserved by transport"), "panic={message}");
}

#[cfg(all(unix, target_os = "linux"))]
#[tokio::test]
async fn connect_transport_stdio_inherits_parent_stderr() {
    let server_cfg = ServerConfig::stdio(vec![
        "sh".to_string(),
        "-c".to_string(),
        "exec cat".to_string(),
    ])
    .unwrap();
    let ctx = ConnectContext {
        trust_mode: TrustMode::Trusted,
        untrusted_streamable_http_policy: UntrustedStreamableHttpPolicy::default(),
        allow_stdout_log_outside_root: false,
        stdout_log_root: None,
        protocol_version: MCP_PROTOCOL_VERSION.to_string(),
        request_timeout: Duration::from_secs(1),
    };

    let (client, child) = connect_transport(&ctx, "srv", &server_cfg, Path::new("/"))
        .await
        .expect("stdio transport should spawn");
    let mut child = child.expect("stdio transport should expose child");
    let pid = child.id().expect("child pid");

    let stderr_path = std::fs::read_link(format!("/proc/{pid}/fd/2")).expect("read stderr fd");
    let parent_stderr_path = std::fs::read_link("/proc/self/fd/2").expect("read parent stderr");
    assert_eq!(stderr_path, parent_stderr_path);

    drop(client);
    child.kill().await.expect("kill child");
    let _ = child.wait().await;
}

#[test]
fn server_handler_timeout_counts_take_resets_counters() {
    let counts = ServerHandlerTimeoutCounts::default();
    let a = ServerName::parse("a").unwrap();
    let b = ServerName::parse("b").unwrap();

    counts
        .counter_for(&a)
        .fetch_add(2, std::sync::atomic::Ordering::Relaxed);
    counts
        .counter_for(&b)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    assert_eq!(counts.count("a"), 2);
    assert_eq!(counts.count("b"), 1);

    let taken = counts.take_and_reset();
    assert_eq!(taken.get("a"), Some(&2));
    assert_eq!(taken.get("b"), Some(&1));

    assert_eq!(counts.count("a"), 0);
    assert_eq!(counts.count("b"), 0);

    let snap = counts.snapshot();
    assert!(!snap.contains_key("a"));
    assert!(!snap.contains_key("b"));
}

#[test]
fn server_handler_timeout_counts_take_keeps_shared_zero_entries() {
    let counts = ServerHandlerTimeoutCounts::default();
    let a = ServerName::parse("a").unwrap();

    let counter = counts.counter_for(&a);
    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let taken = counts.take_and_reset();
    assert_eq!(taken.get("a"), Some(&1));
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 0);

    let snap = counts.snapshot();
    assert_eq!(snap.get("a"), Some(&0));
}

#[test]
fn server_handler_timeout_counts_remove_drops_entry() {
    let counts = ServerHandlerTimeoutCounts::default();
    let a = ServerName::parse("a").unwrap();
    let b = ServerName::parse("b").unwrap();

    counts
        .counter_for(&a)
        .fetch_add(2, std::sync::atomic::Ordering::Relaxed);
    counts
        .counter_for(&b)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    counts.remove("a");

    assert_eq!(counts.count("a"), 0);
    assert_eq!(counts.count("b"), 1);

    let snap = counts.snapshot();
    assert!(!snap.contains_key("a"));
    assert_eq!(snap.get("b"), Some(&1));
}

#[test]
fn disconnect_clears_stale_timeout_counter_without_connection() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);

    let srv = ServerName::parse("srv").unwrap();
    manager
        .server_handler_timeout_counts
        .counter_for(&srv)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(manager.server_handler_timeout_count("srv"), 1);

    assert!(!manager.disconnect("srv"));
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));
}

#[tokio::test]
async fn try_prepare_connected_client_rejects_different_cwd_context() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let _server_stream = server_stream;
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let connected_cwd = std::env::current_dir()
        .expect("current dir")
        .join("workspace")
        .join("a");
    let other_cwd = std::env::current_dir()
        .expect("current dir")
        .join("workspace")
        .join("b");

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let server_name = ServerName::parse("srv").unwrap();
    manager.conns.insert(
        server_name.clone(),
        Connection {
            id: 1,
            child: None,
            client,
            handler_tasks: Vec::new(),
        },
    );
    manager
        .record_connection_cwd("srv", &connected_cwd)
        .unwrap();

    let err = match manager.try_prepare_connected_client("srv", Some(&other_cwd)) {
        Ok(_) => panic!("different cwd should be rejected"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("cannot be reused for cwd="),
        "{err:#}"
    );

    let prepared = manager
        .try_prepare_connected_client("srv", Some(&connected_cwd))
        .unwrap()
        .expect("matching cwd should reuse connection");
    assert_eq!(prepared.server_name, "srv");
}

#[tokio::test]
async fn try_prepare_connected_client_reuses_same_cwd_identity() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let _server_stream = server_stream;
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let connected_cwd = std::env::current_dir()
        .expect("current dir")
        .join("workspace")
        .join("demo");
    let same_cwd_different_spelling = connected_cwd
        .parent()
        .expect("parent")
        .join(".")
        .join("demo");

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let server_name = ServerName::parse("srv").unwrap();
    manager.conns.insert(
        server_name,
        Connection {
            id: 1,
            child: None,
            client,
            handler_tasks: Vec::new(),
        },
    );
    manager
        .record_connection_cwd("srv", &connected_cwd)
        .unwrap();

    let prepared = manager
        .try_prepare_connected_client("srv", Some(&same_cwd_different_spelling))
        .unwrap()
        .expect("stable cwd identity should allow reuse");
    assert_eq!(prepared.server_name, "srv");
}

#[tokio::test]
async fn prepare_transport_connect_rejects_different_cwd_context() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let _server_stream = server_stream;
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let server_name = ServerName::parse("srv").unwrap();
    manager.conns.insert(
        server_name.clone(),
        Connection {
            id: 1,
            child: None,
            client,
            handler_tasks: Vec::new(),
        },
    );
    let connected_cwd = test_workspace_path("a");
    let requested_cwd = test_workspace_path("b");
    manager
        .record_connection_cwd("srv", &connected_cwd)
        .unwrap();

    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        server_name,
        ServerConfig::unix(PathBuf::from("/tmp/mock.sock")).unwrap(),
    );
    let config = Config::new(crate::ClientConfig::default(), servers);

    let err = match manager.prepare_transport_connect(&config, "srv", &requested_cwd) {
        Ok(_) => panic!("different cwd should be rejected"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("cannot be reused for cwd="),
        "{err:#}"
    );
}

#[tokio::test]
async fn prepare_transport_connect_rejects_reuse_without_config_metadata() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let _server_stream = server_stream;
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let server_name = ServerName::parse("srv").unwrap();
    manager.conns.insert(
        server_name.clone(),
        Connection {
            id: 1,
            child: None,
            client,
            handler_tasks: Vec::new(),
        },
    );
    let connected_cwd = test_workspace_path("a");
    manager
        .record_connection_cwd("srv", &connected_cwd)
        .unwrap();

    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        server_name,
        ServerConfig::unix(PathBuf::from("/tmp/mock.sock")).unwrap(),
    );
    let config = Config::new(crate::ClientConfig::default(), servers);

    let err = match manager.prepare_transport_connect(&config, "srv", &connected_cwd) {
        Ok(_) => panic!("config-driven reuse without metadata should fail closed"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("without reusable config metadata"),
        "{err:#}"
    );
}

#[tokio::test]
async fn prepare_transport_connect_rejects_different_effective_config() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let _server_stream = server_stream;
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let server_name = ServerName::parse("srv").unwrap();
    manager.conns.insert(
        server_name.clone(),
        Connection {
            id: 1,
            child: None,
            client,
            handler_tasks: Vec::new(),
        },
    );
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

    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        server_name,
        ServerConfig::unix(PathBuf::from("/tmp/changed.sock")).unwrap(),
    );
    let config = Config::new(crate::ClientConfig::default(), servers);

    let err = match manager.prepare_transport_connect(&config, "srv", &connected_cwd) {
        Ok(_) => panic!("different effective config should not be silently reused"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("different effective config"),
        "{err:#}"
    );
}

#[cfg(not(windows))]
#[test]
fn prepare_transport_connect_resolves_relative_cwd_from_config_thread_root() {
    let tempdir = tempfile::tempdir().unwrap();
    let config_dir = tempdir.path().join("configs");
    std::fs::create_dir_all(&config_dir).unwrap();
    let expected_cwd = config_dir.join("workspace").join("demo");
    let outside = tempfile::tempdir().unwrap();

    let _guard = cwd_test_guard();
    let _cwd_restore = CurrentDirRestoreGuard::capture();
    std::env::set_current_dir(tempdir.path()).expect("enter config dir parent");

    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        ServerName::parse("srv").unwrap(),
        ServerConfig::unix(PathBuf::from("/tmp/mock.sock")).unwrap(),
    );
    let config = Config::new(crate::ClientConfig::default(), servers)
        .with_path(PathBuf::from("configs/mcp.json"));
    std::env::set_current_dir(outside.path()).expect("enter outside dir");

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let prepared = manager
        .prepare_transport_connect(&config, "srv", Path::new("workspace/./demo"))
        .unwrap()
        .expect("transport should prepare");
    assert_eq!(prepared.cwd, expected_cwd);
}

#[cfg(unix)]
#[tokio::test]
async fn disconnect_reaps_child_best_effort() {
    async fn pid_is_alive(pid: u32) -> bool {
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .await
            .is_ok_and(|status| status.success())
    }

    let (client_stream, _server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg("exec sleep 10");
    let child = cmd.spawn().unwrap();
    let child_id = child.id().expect("child id should exist");
    assert!(pid_is_alive(child_id).await);

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let server_name = ServerName::parse("srv").unwrap();
    manager.conns.insert(
        server_name.clone(),
        Connection {
            id: next_connection_id(),
            child: Some(child),
            client,
            handler_tasks: Vec::new(),
        },
    );
    manager
        .init_results
        .insert(server_name, serde_json::json!({ "ok": true }));

    assert!(manager.disconnect("srv"));

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !pid_is_alive(child_id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("disconnect should reap child process in best-effort mode");
}

#[cfg(unix)]
#[tokio::test]
async fn install_failure_reaps_child_best_effort() {
    async fn pid_is_alive(pid: u32) -> bool {
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .await
            .is_ok_and(|status| status.success())
    }

    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        let init_id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": "1900-01-01" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();
    });

    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg("exec sleep 10");
    let child = cmd.spawn().unwrap();
    let child_id = child.id().expect("child id should exist");
    assert!(pid_is_alive(child_id).await);

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let err = manager
        .install_connection_parsed(ServerName::parse("srv").unwrap(), client, Some(child))
        .await
        .expect_err("protocol mismatch should fail install");
    assert!(
        err.to_string().contains("protocolVersion mismatch"),
        "{err:#}"
    );

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !pid_is_alive(child_id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed install should reap child process in best-effort mode");

    server_task.await.unwrap();
}

#[tokio::test]
async fn disconnect_and_wait_clears_stale_timeout_counter_without_connection() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);

    let srv = ServerName::parse("srv").unwrap();
    manager
        .server_handler_timeout_counts
        .counter_for(&srv)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(manager.server_handler_timeout_count("srv"), 1);

    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_millis(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));
}

#[test]
fn disconnect_clears_stale_protocol_mismatch_without_connection() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .protocol_version_mismatches
        .push(ProtocolVersionMismatch {
            server_name: ServerName::parse("srv").unwrap(),
            client_protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            server_protocol_version: "1900-01-01".to_string(),
        });

    assert_eq!(manager.protocol_version_mismatches().len(), 1);
    assert!(!manager.disconnect("srv"));
    assert!(manager.protocol_version_mismatches().is_empty());
}

#[tokio::test]
async fn disconnect_and_wait_clears_stale_protocol_mismatch_without_connection() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .protocol_version_mismatches
        .push(ProtocolVersionMismatch {
            server_name: ServerName::parse("srv").unwrap(),
            client_protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            server_protocol_version: "1900-01-01".to_string(),
        });

    assert_eq!(manager.protocol_version_mismatches().len(), 1);
    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_millis(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());
    assert!(manager.protocol_version_mismatches().is_empty());
}

#[test]
fn expand_placeholders_supports_claude_plugin_root() {
    let cwd = Path::new("/tmp/plugin");
    let expanded = expand_placeholders_trusted("${CLAUDE_PLUGIN_ROOT}/servers/mcp", cwd).unwrap();
    assert_eq!(expanded, "/tmp/plugin/servers/mcp");
}

#[test]
fn expand_placeholders_supports_env_vars() {
    let Ok(path) = std::env::var("PATH") else {
        return;
    };
    let cwd = Path::new("/tmp/plugin");
    let expanded = expand_placeholders_trusted("prefix-${PATH}-suffix", cwd).unwrap();
    assert_eq!(expanded, format!("prefix-{path}-suffix"));
}

#[test]
fn expand_placeholders_rejects_invalid_name() {
    let cwd = Path::new("/tmp/plugin");
    let err = expand_placeholders_trusted("${BAD-NAME}", cwd).unwrap_err();
    assert!(err.to_string().contains("invalid placeholder name"));
}

#[cfg(unix)]
#[test]
fn expand_placeholders_trusted_os_preserves_non_utf8_cwd() {
    let cwd = PathBuf::from(OsString::from_vec(b"/tmp/plugin-\xff".to_vec()));
    let expanded =
        super::placeholders::expand_placeholders_trusted_os("${MCP_ROOT}/server", &cwd).unwrap();

    assert_eq!(expanded.as_os_str().as_bytes(), b"/tmp/plugin-\xff/server");
}

#[cfg(unix)]
#[test]
fn expand_placeholders_trusted_text_rejects_non_utf8_cwd() {
    let cwd = PathBuf::from(OsString::from_vec(b"/tmp/plugin-\xff".to_vec()));
    let err = expand_placeholders_trusted("${CLAUDE_PLUGIN_ROOT}/server", &cwd).unwrap_err();

    assert!(err.to_string().contains("requires a UTF-8 cwd"), "{err:#}");
}

#[tokio::test]
async fn connect_io_performs_initialize_and_exposes_result() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    assert!(manager.is_connected("srv"));
    assert_eq!(
        manager.initialize_result("srv").unwrap(),
        &serde_json::json!({ "hello": "world" })
    );

    server_task.await.unwrap();

    let conn = manager.take_connection("srv");
    assert!(conn.is_some());
    assert!(!manager.is_connected("srv"));
    assert!(manager.initialize_result("srv").is_none());
}

#[tokio::test]
async fn server_request_handler_panic_is_bridged_to_error_response() {
    let (client_stream, server_stream) = tokio::io::duplex(2048);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let sync_panic_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "demo/boom_sync",
            "params": { "x": 1 },
        });
        let mut sync_panic_request_line = serde_json::to_string(&sync_panic_request).unwrap();
        sync_panic_request_line.push('\n');
        server_write
            .write_all(sync_panic_request_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let sync_panic_resp_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let sync_panic_resp_value: Value = serde_json::from_str(&sync_panic_resp_line).unwrap();

        assert_eq!(sync_panic_resp_value["jsonrpc"], "2.0");
        assert_eq!(sync_panic_resp_value["id"], 42);
        assert_eq!(sync_panic_resp_value["error"]["code"], -32000);
        assert!(
            sync_panic_resp_value["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("panicked"),
            "{sync_panic_resp_value}"
        );

        let async_panic_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 43,
            "method": "demo/boom",
            "params": { "x": 2 },
        });
        let mut async_panic_request_line = serde_json::to_string(&async_panic_request).unwrap();
        async_panic_request_line.push('\n');
        server_write
            .write_all(async_panic_request_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let async_panic_resp_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let async_panic_resp_value: Value = serde_json::from_str(&async_panic_resp_line).unwrap();

        assert_eq!(async_panic_resp_value["jsonrpc"], "2.0");
        assert_eq!(async_panic_resp_value["id"], 43);
        assert_eq!(async_panic_resp_value["error"]["code"], -32000);
        assert!(
            async_panic_resp_value["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("panicked"),
            "{async_panic_resp_value}"
        );

        let ok_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 44,
            "method": "demo/ok",
            "params": { "x": 3 },
        });
        let mut ok_request_line = serde_json::to_string(&ok_request).unwrap();
        ok_request_line.push('\n');
        server_write
            .write_all(ok_request_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let ok_resp_line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let ok_resp_value: Value = serde_json::from_str(&ok_resp_line).unwrap();
        assert_eq!(ok_resp_value["jsonrpc"], "2.0");
        assert_eq!(ok_resp_value["id"], 44);
        assert_eq!(ok_resp_value["result"], serde_json::json!({ "ok": true }));
    });

    let handler: ServerRequestHandler = Arc::new(|ctx| {
        if ctx.method == "demo/boom_sync" {
            panic!("boom sync");
        }
        Box::pin(async move {
            match ctx.method.as_str() {
                "demo/boom" => panic!("boom"),
                "demo/ok" => ServerRequestOutcome::Ok(serde_json::json!({ "ok": true })),
                _ => ServerRequestOutcome::MethodNotFound,
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

    server_task.await.unwrap();
    assert!(manager.take_connection("srv").is_some());
}

#[tokio::test]
async fn request_connected_disconnects_after_protocol_error() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let init_id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let ping_line = lines.next_line().await.unwrap().unwrap();
        let ping_value: Value = serde_json::from_str(&ping_line).unwrap();
        assert_eq!(ping_value["jsonrpc"], "2.0");
        assert_eq!(ping_value["method"], "ping");
        let ping_id = ping_value["id"].clone();

        // Send an intentionally malformed JSON-RPC response (wrong jsonrpc version)
        // to trigger a protocol error without necessarily closing the transport.
        let response = serde_json::json!({
            "jsonrpc": "1.0",
            "id": ping_id,
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

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(1))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();
    seed_manager_side_state(&mut manager, "srv");

    let err = manager
        .request_connected("srv", "ping", None)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("mcp request failed: ping (server=srv)")
    );

    // Connection is dropped after Protocol/Io errors to avoid keeping a stale/broken client.
    assert!(!manager.is_connected("srv"));
    assert!(manager.initialize_result("srv").is_none());
    assert!(manager.protocol_version_mismatches().is_empty());
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));

    server_task.await.unwrap();
}

#[tokio::test]
async fn request_connected_accepts_trimmed_server_name() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let init_id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let ping_line = lines.next_line().await.unwrap().unwrap();
        let ping_value: Value = serde_json::from_str(&ping_line).unwrap();
        assert_eq!(ping_value["jsonrpc"], "2.0");
        assert_eq!(ping_value["method"], "ping");
        let ping_id = ping_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": ping_id,
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
        assert!(eof.is_none(), "expected EOF after disconnect");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(1))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io(" srv ", client_read, client_write)
        .await
        .unwrap();

    assert!(manager.is_connected("srv"));
    assert!(manager.is_connected(" srv "));
    assert_eq!(
        manager.initialize_result(" srv ").unwrap(),
        &serde_json::json!({ "hello": "world" })
    );
    assert_eq!(
        manager
            .request_connected(" srv ", "ping", None)
            .await
            .unwrap(),
        serde_json::json!({ "ok": true })
    );

    let status = manager
        .disconnect_and_wait(
            " srv ",
            Duration::from_secs(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());

    server_task.await.unwrap();
}

#[tokio::test]
async fn request_connected_timeout_late_response_does_not_disconnect() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let init_id = init_value["id"].clone();

        let init_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut init_response_line = serde_json::to_string(&init_response).unwrap();
        init_response_line.push('\n');
        server_write
            .write_all(init_response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let slow_line = lines.next_line().await.unwrap().unwrap();
        let slow_value: Value = serde_json::from_str(&slow_line).unwrap();
        assert_eq!(slow_value["method"], "slow");
        let slow_id = slow_value["id"].clone();

        tokio::time::sleep(Duration::from_millis(80)).await;
        let slow_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": slow_id,
            "result": { "slow": true },
        });
        let mut slow_response_line = serde_json::to_string(&slow_response).unwrap();
        slow_response_line.push('\n');
        server_write
            .write_all(slow_response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let fast_line = lines.next_line().await.unwrap().unwrap();
        let fast_value: Value = serde_json::from_str(&fast_line).unwrap();
        assert_eq!(fast_value["method"], "fast");
        let fast_id = fast_value["id"].clone();

        let fast_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": fast_id,
            "result": { "ok": true },
        });
        let mut fast_response_line = serde_json::to_string(&fast_response).unwrap();
        fast_response_line.push('\n');
        server_write
            .write_all(fast_response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after disconnect");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_millis(20))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    let err = manager
        .request_connected("srv", "slow", None)
        .await
        .expect_err("slow request should time out");
    assert!(err.to_string().contains("timed out"));

    tokio::time::sleep(Duration::from_millis(120)).await;

    let fast = manager
        .request_connected("srv", "fast", None)
        .await
        .expect("connection should remain usable after late response");
    assert_eq!(fast, serde_json::json!({ "ok": true }));
    assert!(manager.is_connected("srv"));

    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_secs(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());

    server_task.await.unwrap();
}

#[tokio::test]
async fn connect_io_session_returns_session_and_supports_requests() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");
        assert!(note_value.get("params").is_none());

        let ping_line = lines.next_line().await.unwrap().unwrap();
        let ping_value: Value = serde_json::from_str(&ping_line).unwrap();
        assert_eq!(ping_value["jsonrpc"], "2.0");
        assert_eq!(ping_value["method"], "ping");
        assert!(ping_value.get("params").is_none());
        let ping_id = ping_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": ping_id,
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
    let session = manager
        .connect_io_session("srv", client_read, client_write)
        .await
        .unwrap();

    assert!(!manager.is_connected("srv"));
    assert_eq!(
        session.initialize_result(),
        &serde_json::json!({ "hello": "world" })
    );
    assert_eq!(
        session
            .request_typed::<crate::mcp::PingRequest>(None)
            .await
            .unwrap(),
        serde_json::json!({ "ok": true })
    );

    server_task.await.unwrap();
}

#[tokio::test]
async fn session_notify_timeout_is_bounded_when_close_path_blocks() {
    use std::io;
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
        ) -> Poll<io::Result<usize>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }
    }

    let entered = Arc::new(AtomicBool::new(false));
    let (client_stream, _server_stream) = tokio::io::duplex(64);
    let (client_read, _client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(
        client_read,
        BlockingWrite {
            entered: entered.clone(),
        },
    )
    .await
    .unwrap();

    let connection = Connection {
        id: next_connection_id(),
        child: None,
        client,
        handler_tasks: Vec::new(),
    };
    let session = crate::Session::new(
        ServerName::parse("srv").unwrap(),
        connection,
        serde_json::json!({}),
        Duration::from_millis(20),
    );

    let started = tokio::time::Instant::now();
    let err = session
        .notify("demo/notify", Some(serde_json::json!({ "x": 1 })))
        .await
        .expect_err("notify should time out");

    assert!(entered.load(Ordering::Relaxed));
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "notify timeout should be bounded, elapsed={:?}",
        started.elapsed()
    );
    assert!(
        contains_wait_timeout(err.as_anyhow()),
        "timeout should preserve structured wait-timeout error, err={err:#}"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn session_notify_timeout_returns_without_waiting_for_close_budget() {
    use std::io;
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
        ) -> Poll<io::Result<usize>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }
    }

    let entered = Arc::new(AtomicBool::new(false));
    let (client_stream, _server_stream) = tokio::io::duplex(64);
    let (client_read, _client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(
        client_read,
        BlockingWrite {
            entered: entered.clone(),
        },
    )
    .await
    .unwrap();

    let connection = Connection {
        id: next_connection_id(),
        child: None,
        client,
        handler_tasks: Vec::new(),
    };
    let session = crate::Session::new(
        ServerName::parse("srv").unwrap(),
        connection,
        serde_json::json!({}),
        Duration::from_millis(20),
    );

    let started = tokio::time::Instant::now();
    let err = session
        .notify("demo/notify", Some(serde_json::json!({ "x": 1 })))
        .await
        .expect_err("notify should time out");

    assert!(entered.load(Ordering::Relaxed));
    assert!(
        started.elapsed() < Duration::from_millis(30),
        "notify timeout should not wait for close budget, elapsed={:?}",
        started.elapsed()
    );
    assert!(
        contains_wait_timeout(err.as_anyhow()),
        "timeout should preserve structured wait-timeout error, err={err:#}"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn session_notify_timeout_leaves_client_open_for_retry() {
    use std::io;
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
        ) -> Poll<io::Result<usize>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }
    }

    let entered = Arc::new(AtomicBool::new(false));
    let (client_stream, _server_stream) = tokio::io::duplex(64);
    let (client_read, _client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(
        client_read,
        BlockingWrite {
            entered: entered.clone(),
        },
    )
    .await
    .unwrap();

    let connection = Connection {
        id: next_connection_id(),
        child: None,
        client,
        handler_tasks: Vec::new(),
    };
    let session = crate::Session::new(
        ServerName::parse("srv").unwrap(),
        connection,
        serde_json::json!({}),
        Duration::from_millis(20),
    );

    let first_err = session
        .notify("demo/first", Some(serde_json::json!({ "x": 1 })))
        .await
        .expect_err("first notify should time out");
    assert!(entered.load(Ordering::Relaxed));
    assert!(
        contains_wait_timeout(first_err.as_anyhow()),
        "timeout should preserve structured wait-timeout error, err={first_err:#}"
    );

    let handle = session.connection().client().handle();
    assert!(
        !handle.is_closed(),
        "timeout should not mark the client closed"
    );
    assert_eq!(
        handle.close_reason(),
        None,
        "timeout should not stamp a close reason when the client stays open"
    );

    let second_started = tokio::time::Instant::now();
    let second_err = session
        .notify("demo/second", Some(serde_json::json!({ "x": 2 })))
        .await
        .expect_err("second notify should time out again");
    assert!(
        second_started.elapsed() < Duration::from_millis(30),
        "repeated timeout should still stay bounded, elapsed={:?}",
        second_started.elapsed()
    );
    assert!(
        contains_wait_timeout(second_err.as_anyhow()),
        "second notify should keep surfacing wait-timeout, err={second_err:#}"
    );
    assert_eq!(
        session.connection().client().handle().close_reason(),
        None,
        "repeated timeouts should not invent a close reason"
    );
}

#[tokio::test]
async fn session_notify_timeout_keeps_background_reader_task_running() {
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

    struct BlockingWrite;

    impl tokio::io::AsyncWrite for BlockingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    let reader_dropped = Arc::new(AtomicBool::new(false));
    let client = mcp_jsonrpc::Client::connect_io(
        BlockingRead {
            dropped: Arc::clone(&reader_dropped),
        },
        BlockingWrite,
    )
    .await
    .unwrap();

    let connection = Connection {
        id: next_connection_id(),
        child: None,
        client,
        handler_tasks: Vec::new(),
    };
    let session = crate::Session::new(
        ServerName::parse("srv").unwrap(),
        connection,
        serde_json::json!({}),
        Duration::from_millis(20),
    );

    let err = session
        .notify("demo/notify", Some(serde_json::json!({ "x": 1 })))
        .await
        .expect_err("notify should time out");
    assert!(
        contains_wait_timeout(err.as_anyhow()),
        "timeout should preserve structured wait-timeout error, err={err:#}"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !reader_dropped.load(Ordering::Relaxed),
        "timeout should not abort the background reader task"
    );
}

#[tokio::test]
async fn connect_io_session_without_handler_timeout_does_not_track_timeout_counter() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let session = manager
        .connect_io_session("srv", client_read, client_write)
        .await
        .unwrap();

    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));

    session.wait().await.unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn session_request_timeout_late_response_keeps_session_usable() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let init_id = init_value["id"].clone();

        let init_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut init_response_line = serde_json::to_string(&init_response).unwrap();
        init_response_line.push('\n');
        server_write
            .write_all(init_response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let slow_line = lines.next_line().await.unwrap().unwrap();
        let slow_value: Value = serde_json::from_str(&slow_line).unwrap();
        assert_eq!(slow_value["method"], "slow");
        let slow_id = slow_value["id"].clone();

        tokio::time::sleep(Duration::from_millis(80)).await;
        let slow_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": slow_id,
            "result": { "slow": true },
        });
        let mut slow_response_line = serde_json::to_string(&slow_response).unwrap();
        slow_response_line.push('\n');
        server_write
            .write_all(slow_response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let fast_line = lines.next_line().await.unwrap().unwrap();
        let fast_value: Value = serde_json::from_str(&fast_line).unwrap();
        assert_eq!(fast_value["method"], "fast");
        let fast_id = fast_value["id"].clone();

        let fast_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": fast_id,
            "result": { "ok": true },
        });
        let mut fast_response_line = serde_json::to_string(&fast_response).unwrap();
        fast_response_line.push('\n');
        server_write
            .write_all(fast_response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after session wait");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let session = manager
        .connect_io_session("srv", client_read, client_write)
        .await
        .unwrap()
        .with_timeout(Duration::from_millis(20));

    let err = session
        .request("slow", None)
        .await
        .expect_err("slow session request should time out");
    assert!(err.to_string().contains("timed out"));

    tokio::time::sleep(Duration::from_millis(120)).await;

    let fast = session
        .request("fast", None)
        .await
        .expect("session should remain usable after late response");
    assert_eq!(fast, serde_json::json!({ "ok": true }));

    let status = session.wait().await.unwrap();
    assert!(status.is_none());
    server_task.await.unwrap();
}

#[tokio::test]
async fn notify_connected_timeout_disconnects_connection() {
    let (client_stream, server_stream) = tokio::io::duplex(256);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let init_id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_millis(20))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();
    seed_manager_side_state(&mut manager, "srv");

    let err = manager
        .notify_connected(
            "srv",
            "demo/notify",
            Some(serde_json::json!({ "blob": "x".repeat(256 * 1024) })),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("timed out"),
        "unexpected error: {err:#}"
    );
    assert!(
        !manager.is_connected("srv"),
        "notification timeout should close potentially corrupted connection"
    );
    assert!(manager.protocol_version_mismatches().is_empty());
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));

    server_task.await.unwrap();
}

#[tokio::test]
async fn session_notify_timeout_keeps_client_open() {
    let (client_stream, server_stream) = tokio::io::duplex(256);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let init_id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let init_note = lines.next_line().await.unwrap().unwrap();
        let init_note_value: Value = serde_json::from_str(&init_note).unwrap();
        assert_eq!(init_note_value["method"], "notifications/initialized");

        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_millis(20))
        .with_trust_mode(TrustMode::Trusted);
    let session = manager
        .connect_io_session("srv", client_read, client_write)
        .await
        .unwrap()
        .with_timeout(Duration::from_millis(20));

    let err = session
        .notify(
            "demo/notify",
            Some(serde_json::json!({ "blob": "x".repeat(256 * 1024) })),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("timed out"),
        "unexpected error: {err:#}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !session.connection().client().handle().is_closed(),
        "notify timeout should not close the session client"
    );

    server_task.await.unwrap();
}

#[tokio::test]
async fn connect_io_rejects_initialize_protocol_version_mismatch() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": "1900-01-01" },
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
    let err = match manager
        .connect_io_session("srv", client_read, client_write)
        .await
    {
        Ok(_) => panic!("expected protocolVersion mismatch"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("protocolVersion mismatch"));
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(
        !manager.server_handler_timeout_counts().contains_key("srv"),
        "failed initialize should not retain timeout counter entry"
    );

    server_task.await.unwrap();
}

#[tokio::test]
async fn connect_io_rejects_missing_initialize_protocol_version_in_strict_mode() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "hello": "world" },
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
    let err = match manager
        .connect_io_session("srv", client_read, client_write)
        .await
    {
        Ok(_) => panic!("missing protocolVersion should fail in strict mode"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("missing string protocolVersion"),
        "unexpected error: {err:#}"
    );

    server_task.await.unwrap();
}

#[tokio::test]
async fn connect_io_allows_initialize_protocol_version_mismatch_when_configured() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);

    {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["jsonrpc"], "2.0");
            assert_eq!(init_value["method"], "initialize");
            let id = init_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": "1900-01-01", "hello": "world" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let note_line = lines.next_line().await.unwrap().unwrap();
            let note_value: Value = serde_json::from_str(&note_line).unwrap();
            assert_eq!(note_value["jsonrpc"], "2.0");
            assert_eq!(note_value["method"], "notifications/initialized");
        });

        let session = manager
            .connect_io_session("srv", client_read, client_write)
            .await
            .unwrap();
        assert_eq!(
            session.initialize_result(),
            &serde_json::json!({ "hello": "world" })
        );
        assert_eq!(manager.protocol_version_mismatches().len(), 1);
        assert_eq!(
            manager.protocol_version_mismatches()[0],
            ProtocolVersionMismatch {
                server_name: ServerName::parse("srv").unwrap(),
                client_protocol_version: MCP_PROTOCOL_VERSION.to_string(),
                server_protocol_version: "1900-01-01".to_string(),
            }
        );

        session.wait().await.unwrap();
        server_task.await.unwrap();
    }

    // A second connection should not grow the mismatch list unboundedly.
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": "1900-01-01", "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");
    });

    let session = manager
        .connect_io_session("srv", client_read, client_write)
        .await
        .unwrap();
    assert_eq!(manager.protocol_version_mismatches().len(), 1);
    session.wait().await.unwrap();

    server_task.await.unwrap();
}

#[tokio::test]
async fn initialize_failure_clears_protocol_mismatch_state() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": "1900-01-01" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();
        drop(server_write);
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);
    seed_manager_side_state(&mut manager, "srv");
    let err = match manager
        .connect_io_session("srv", client_read, client_write)
        .await
    {
        Ok(_) => panic!("expected connect_io_session failure"),
        Err(err) => err,
    };
    let err_chain = format!("{err:#}");
    assert!(
        err_chain.contains("mcp initialized notification failed"),
        "unexpected error: {err_chain}"
    );
    assert!(
        manager.protocol_version_mismatches().is_empty(),
        "failed initialize should not retain mismatch state"
    );
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));

    server_task.await.unwrap();
}

#[tokio::test]
async fn protocol_version_mismatch_is_cleared_after_matching_reconnect() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);

    {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let id = init_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": "1900-01-01" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let note_line = lines.next_line().await.unwrap().unwrap();
            let note_value: Value = serde_json::from_str(&note_line).unwrap();
            assert_eq!(note_value["method"], "notifications/initialized");
        });

        let session = manager
            .connect_io_session("srv", client_read, client_write)
            .await
            .unwrap();
        assert_eq!(manager.protocol_version_mismatches().len(), 1);
        session.wait().await.unwrap();
        server_task.await.unwrap();
    }

    {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let id = init_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let note_line = lines.next_line().await.unwrap().unwrap();
            let note_value: Value = serde_json::from_str(&note_line).unwrap();
            assert_eq!(note_value["method"], "notifications/initialized");
        });

        let session = manager
            .connect_io_session("srv", client_read, client_write)
            .await
            .unwrap();
        assert!(
            manager.protocol_version_mismatches().is_empty(),
            "matching reconnect should clear stale mismatch entry"
        );
        session.wait().await.unwrap();
        server_task.await.unwrap();
    }
}

#[tokio::test]
async fn protocol_version_mismatch_is_cleared_when_reconnect_omits_protocol_version() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);

    {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let id = init_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": "1900-01-01" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let note_line = lines.next_line().await.unwrap().unwrap();
            let note_value: Value = serde_json::from_str(&note_line).unwrap();
            assert_eq!(note_value["method"], "notifications/initialized");
        });

        let session = manager
            .connect_io_session("srv", client_read, client_write)
            .await
            .unwrap();
        assert_eq!(manager.protocol_version_mismatches().len(), 1);
        session.wait().await.unwrap();
        server_task.await.unwrap();
    }

    {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);

        let server_task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(server_read).lines();

            let init_line = lines.next_line().await.unwrap().unwrap();
            let init_value: Value = serde_json::from_str(&init_line).unwrap();
            assert_eq!(init_value["method"], "initialize");
            let id = init_value["id"].clone();

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_write
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let note_line = lines.next_line().await.unwrap().unwrap();
            let note_value: Value = serde_json::from_str(&note_line).unwrap();
            assert_eq!(note_value["method"], "notifications/initialized");
        });

        let session = manager
            .connect_io_session("srv", client_read, client_write)
            .await
            .unwrap();
        assert!(
            manager.protocol_version_mismatches().is_empty(),
            "reconnect without protocolVersion should clear stale mismatch entry"
        );
        session.wait().await.unwrap();
        server_task.await.unwrap();
    }
}

#[tokio::test]
async fn disconnect_clears_protocol_version_mismatch_entry() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": "1900-01-01" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after disconnect");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();
    assert_eq!(manager.protocol_version_mismatches().len(), 1);

    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_secs(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());
    assert!(
        manager.protocol_version_mismatches().is_empty(),
        "disconnect should clear mismatch entry"
    );

    server_task.await.unwrap();
}

#[tokio::test]
async fn take_connection_clears_protocol_version_mismatch_entry() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": "1900-01-01" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after take_connection wait");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();
    assert_eq!(manager.protocol_version_mismatches().len(), 1);

    let conn = manager
        .take_connection("srv")
        .expect("connection should exist");
    assert!(
        manager.protocol_version_mismatches().is_empty(),
        "take_connection should clear mismatch entry"
    );

    let status = conn.wait().await.unwrap();
    assert!(status.is_none());
    server_task.await.unwrap();
}

#[tokio::test]
async fn take_session_and_clear_state_clears_manager_state() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": "1900-01-01" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(
            eof.is_none(),
            "expected EOF after take_session_and_clear_state wait"
        );
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_protocol_version_check(ProtocolVersionCheck::Warn);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();
    assert_eq!(manager.protocol_version_mismatches().len(), 1);

    let srv = ServerName::parse("srv").unwrap();
    manager
        .server_handler_timeout_counts
        .counter_for(&srv)
        .fetch_add(2, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(manager.server_handler_timeout_count("srv"), 2);

    let session = manager
        .take_session_and_clear_state(" srv ")
        .expect("session should exist");
    assert!(
        manager.protocol_version_mismatches().is_empty(),
        "take_session_and_clear_state should clear mismatch entry"
    );
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));

    let status = session.wait().await.unwrap();
    assert!(status.is_none());
    server_task.await.unwrap();
}

#[test]
fn take_session_and_clear_state_without_connection_clears_stale_state() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .protocol_version_mismatches
        .push(ProtocolVersionMismatch {
            server_name: ServerName::parse("srv").unwrap(),
            client_protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            server_protocol_version: "1900-01-01".to_string(),
        });

    let srv = ServerName::parse("srv").unwrap();
    manager
        .server_handler_timeout_counts
        .counter_for(&srv)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(manager.server_handler_timeout_count("srv"), 1);

    assert!(manager.take_session_and_clear_state("srv").is_none());
    assert!(manager.protocol_version_mismatches().is_empty());
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));
}

#[tokio::test]
async fn take_session_keeps_connection_when_init_result_is_missing() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after explicit connection wait");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();
    assert!(manager.is_connected("srv"));

    manager.init_results.remove("srv");
    assert!(
        manager.take_session("srv").is_none(),
        "take_session should fail when initialize result is missing"
    );
    assert!(
        manager.is_connected("srv"),
        "connection should remain cached when initialize result is missing"
    );

    let conn = manager
        .take_connection("srv")
        .expect("connection should remain available");
    let status = conn.wait().await.unwrap();
    assert!(status.is_none());

    server_task.await.unwrap();
}

#[tokio::test]
async fn server_notification_handler_timeout_is_counted() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let note = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "demo/notify",
            "params": {},
        });
        let mut note_line = serde_json::to_string(&note).unwrap();
        note_line.push('\n');
        server_write.write_all(note_line.as_bytes()).await.unwrap();
        server_write.flush().await.unwrap();
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted)
        .with_server_notification_handler(Arc::new(|_ctx| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
        }))
        .with_server_handler_timeout(Duration::from_millis(10));
    let session = manager
        .connect_io_session("srv", client_read, client_write)
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if manager.server_handler_timeout_count("srv") >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    session.wait().await.unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn connect_io_reconnects_when_existing_connection_is_closed() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    server_task.await.unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if manager
                .conns
                .get("srv")
                .expect("srv conn exists")
                .client
                .handle()
                .is_closed()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("client marked closed");

    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "hello": "world" },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");
    });

    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("server task completed")
        .expect("server task ok");
}

#[tokio::test]
async fn connect_io_with_spaced_name_rejects_duplicate_live_connection() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server1_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "marker": 1 },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after disconnect");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io(" srv ", client_read, client_write)
        .await
        .unwrap();
    assert_eq!(
        manager.initialize_result("srv").unwrap()["marker"],
        serde_json::json!(1)
    );

    let (client_stream2, server_stream2) = tokio::io::duplex(1024);
    let (client_read2, client_write2) = tokio::io::split(client_stream2);
    let (server_read2, mut server_write2) = tokio::io::split(server_stream2);

    let server2_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read2).lines();
        let next = tokio::time::timeout(Duration::from_millis(500), lines.next_line()).await;
        let Ok(Ok(Some(init_line))) = next else {
            return false;
        };

        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        if init_value["method"] != "initialize" {
            return false;
        }
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "marker": 2 },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write2
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write2.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");
        true
    });

    let err = manager
        .connect_io(" srv ", client_read2, client_write2)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already connected"), "{err}");
    assert!(
        err.to_string().contains("custom JSON-RPC IO transport"),
        "{err}"
    );

    assert_eq!(
        manager.initialize_result("srv").unwrap()["marker"],
        serde_json::json!(1)
    );
    let saw_second_initialize = server2_task.await.unwrap();
    assert!(
        !saw_second_initialize,
        "duplicate connection should not send initialize when normalized name is already connected"
    );

    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_secs(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());
    server1_task.await.unwrap();
}

#[tokio::test]
async fn connect_io_unchecked_rejects_invalid_server_name() {
    let (client_stream, _server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    let err = manager
        .connect_io_unchecked(" bad name! ", client_read, client_write)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid mcp server name"));
}

#[tokio::test]
async fn untrusted_manager_refuses_stdio_spawn() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::stdio(vec!["mcp-server".to_string()]).unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("untrusted mode"));
}

#[tokio::test]
async fn untrusted_manager_refuses_custom_jsonrpc_attachments() {
    let (client_stream, _server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);

    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let err = manager
        .connect_jsonrpc("srv", client)
        .await
        .expect_err("should refuse in untrusted mode");
    assert!(err.to_string().contains("untrusted mode"));
    assert!(err.to_string().contains("connect_jsonrpc_unchecked"));

    let (client_stream, _server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let err = manager
        .connect_io("srv2", client_read, client_write)
        .await
        .expect_err("should refuse in untrusted mode");
    assert!(err.to_string().contains("untrusted mode"));
    assert!(err.to_string().contains("connect_io_unchecked"));
}

#[cfg(unix)]
#[tokio::test]
async fn connect_jsonrpc_rejects_duplicate_live_connection_and_reaps_rejected_child() {
    async fn pid_is_alive(pid: u32) -> bool {
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .await
            .is_ok_and(|status| status.success())
    }

    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "marker": 1 },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after disconnect");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg("exec sleep 10");
    let duplicate_client = mcp_jsonrpc::Client::spawn_command(cmd).await.unwrap();
    let child_id = duplicate_client
        .child_id()
        .expect("duplicate client child pid should exist");
    assert!(pid_is_alive(child_id).await);

    let err = manager
        .connect_jsonrpc("srv", duplicate_client)
        .await
        .expect_err("duplicate custom client should be rejected");
    let message = err.to_string();
    assert!(message.contains("already connected"), "{message}");
    assert!(message.contains("disconnect first"), "{message}");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !pid_is_alive(child_id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("rejected duplicate client child should be reaped");

    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_secs(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());
    server_task.await.unwrap();
}

#[tokio::test]
async fn untrusted_manager_refuses_unix_connect() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::unix(PathBuf::from("/tmp/mcp.sock")).unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("untrusted mode"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_env_secrets() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
    server_cfg
        .set_bearer_token_env_var(Some("MCP_TOKEN".to_string()))
        .unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("bearer token env var"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_env_header_secrets() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
    server_cfg
        .env_http_headers_mut()
        .unwrap()
        .insert("x-api-key".to_string(), "MCP_API_KEY".to_string());

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("http header env vars"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_auth_like_static_headers() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    for header in ["X-Api-Key", "X-Auth-Token", "X-Client-Secret"] {
        let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
        server_cfg
            .http_headers_mut()
            .unwrap()
            .insert(header.to_string(), "secret".to_string());

        let err = manager
            .connect("srv", &server_cfg, absolute_test_cwd())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("sensitive http header"),
            "header={header} err={err}"
        );
    }
}

#[tokio::test]
async fn session_notify_timeout_does_not_close_client() {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct SwitchableWrite {
        blocked: Arc<AtomicBool>,
        init_seen: Arc<tokio::sync::Notify>,
    }

    impl tokio::io::AsyncWrite for SwitchableWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if self.blocked.load(Ordering::Relaxed) {
                return Poll::Pending;
            }
            self.init_seen.notify_one();
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            if self.blocked.load(Ordering::Relaxed) {
                return Poll::Pending;
            }
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    let (client_stream, mut server_stream) = tokio::io::duplex(1024);
    let (client_read, _unused_client_write) = tokio::io::split(client_stream);
    let blocked = Arc::new(AtomicBool::new(false));
    let init_seen = Arc::new(tokio::sync::Notify::new());

    let server_task = {
        let init_seen = Arc::clone(&init_seen);
        tokio::spawn(async move {
            init_seen.notified().await;
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "protocolVersion": MCP_PROTOCOL_VERSION }
            });
            let mut response_line = serde_json::to_string(&response).unwrap();
            response_line.push('\n');
            server_stream
                .write_all(response_line.as_bytes())
                .await
                .unwrap();
            server_stream.flush().await.unwrap();
            std::future::pending::<()>().await;
        })
    };

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_millis(20))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io(
            "srv",
            client_read,
            SwitchableWrite {
                blocked: Arc::clone(&blocked),
                init_seen: Arc::clone(&init_seen),
            },
        )
        .await
        .unwrap();

    let session = manager.take_session("srv").expect("take session");
    blocked.store(true, Ordering::Relaxed);

    let err = session
        .notify("notifications/progress", None)
        .await
        .expect_err("notify should time out");
    assert!(
        err.to_string().contains("timed out"),
        "unexpected notify timeout error: {err}"
    );
    assert!(
        !session.connection().client().is_closed(),
        "notify timeout should not implicitly close the session client"
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_non_https_urls() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("http://example.com/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("non-https"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_localhost() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://localhost/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("localhost"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_localdomain() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://localhost.localdomain/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("localdomain"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_single_label_hosts() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://example/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("single-label"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_private_ip() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://192.168.0.10/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("non-global ip"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_ipv4_mapped_ipv6_loopback() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://[::ffff:127.0.0.1]/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("non-global ip"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_nat64_well_known_prefix_private_ip() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://[64:ff9b::c0a8:0001]/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("non-global ip"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_6to4_private_ip() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://[2002:c0a8:0001::]/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("non-global ip"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_discard_only_ipv6_prefix() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://[100::1]/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("non-global ip"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_ipv6_benchmark_prefix() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://[2001:2::1]/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("non-global ip"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_url_credentials() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5));
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://user:pass@example.com/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("url credentials"));
}

#[tokio::test]
async fn untrusted_manager_refuses_streamable_http_hostname_resolving_to_non_global_ip_by_default()
{
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allow_localhost: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_untrusted_streamable_http_policy(policy);
    assert_eq!(manager.trust_mode(), TrustMode::Untrusted);

    let server_cfg = ServerConfig::streamable_http("https://localhost/mcp").unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("resolves to non-global ip"),
        "unexpected error: {err}"
    );
}

#[test]
fn streamable_http_validate_rejects_reserved_authorization_env_header() {
    let mut server_cfg = ServerConfig::streamable_http("https://example.com/mcp").unwrap();
    server_cfg
        .env_http_headers_mut()
        .unwrap()
        .insert("Authorization".to_string(), "MCP_TOKEN".to_string());

    let err = server_cfg.validate().unwrap_err();
    assert!(err.to_string().contains("reserved by transport"));
}

#[test]
fn untrusted_policy_allows_http_when_configured() {
    let policy = UntrustedStreamableHttpPolicy {
        require_https: false,
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "http://example.com/mcp")
        .unwrap();
}

#[test]
fn untrusted_policy_allows_private_ip_when_configured() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allow_private_ips: true,
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://192.168.0.10/mcp")
        .unwrap();
}

#[test]
fn untrusted_policy_allow_private_ip_does_not_allow_localhost_hostnames_by_itself() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allow_private_ips: true,
            ..Default::default()
        },
        ..Default::default()
    };

    let err =
        validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://localhost/mcp")
            .unwrap_err();
    assert!(err.to_string().contains("localhost/local/single-label"));
}

#[test]
fn untrusted_policy_allows_nat64_well_known_prefix_when_embedded_ipv4_is_public() {
    let policy = UntrustedStreamableHttpPolicy::default();

    validate_streamable_http_url_untrusted(
        &policy,
        "srv",
        "url",
        "https://[64:ff9b::0808:0808]/mcp",
    )
    .unwrap();
}

#[test]
fn untrusted_policy_allows_6to4_when_embedded_ipv4_is_public() {
    let policy = UntrustedStreamableHttpPolicy::default();

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://[2002:0808:0808::]/mcp")
        .unwrap();
}

#[test]
fn untrusted_policy_enforces_allowlist_when_set() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allowed_hosts: vec!["example.com".to_string()],
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://example.com/mcp")
        .unwrap();
    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://api.example.com/mcp")
        .unwrap();

    let err = validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://evil.com/mcp")
        .unwrap_err();
    assert!(err.to_string().contains("allowlist"));
}

#[test]
fn untrusted_policy_allow_localhost_does_not_allow_local_domains_or_single_label_hosts() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allow_localhost: true,
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://localhost/mcp").unwrap();
    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://demo.localhost/mcp")
        .unwrap();

    let err =
        validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://service.local/mcp")
            .unwrap_err();
    assert!(err.to_string().contains("localhost/local/single-label"));

    let err = validate_streamable_http_url_untrusted(
        &policy,
        "srv",
        "url",
        "https://service.localdomain/mcp",
    )
    .unwrap_err();
    assert!(err.to_string().contains("localhost/local/single-label"));

    let err = validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://internal/mcp")
        .unwrap_err();
    assert!(err.to_string().contains("localhost/local/single-label"));
}

#[tokio::test]
async fn untrusted_policy_dns_check_blocks_localhost_without_allow_private_ip() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allow_localhost: true,
            dns_check: true,
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://localhost/mcp").unwrap();
    let err =
        validate_streamable_http_url_untrusted_dns(&policy, "srv", "url", "https://localhost/mcp")
            .await
            .unwrap_err();
    assert!(err.to_string().contains("resolves to non-global ip"));
}

#[tokio::test]
async fn untrusted_policy_dns_check_allows_localhost_with_allow_private_ip() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            allow_localhost: true,
            allow_private_ips: true,
            dns_check: true,
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(&policy, "srv", "url", "https://localhost/mcp").unwrap();
    validate_streamable_http_url_untrusted_dns(&policy, "srv", "url", "https://localhost/mcp")
        .await
        .unwrap();
}

#[tokio::test]
async fn untrusted_policy_dns_check_fails_closed_on_lookup_failure_or_timeout() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            dns_check: true,
            dns_timeout: Duration::from_nanos(1),
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(
        &policy,
        "srv",
        "url",
        "https://does-not-exist.invalid/mcp",
    )
    .unwrap();
    let err = validate_streamable_http_url_untrusted_dns(
        &policy,
        "srv",
        "url",
        "https://does-not-exist.invalid/mcp",
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("timed out dns lookup")
            || err.to_string().contains("failed dns lookup"),
        "err={err}"
    );
}

#[tokio::test]
async fn untrusted_policy_dns_check_can_fail_open_on_lookup_timeout() {
    let policy = UntrustedStreamableHttpPolicy {
        outbound: http_kit::UntrustedOutboundPolicy {
            dns_check: true,
            dns_fail_open: true,
            dns_timeout: Duration::from_nanos(1),
            ..Default::default()
        },
        ..Default::default()
    };

    validate_streamable_http_url_untrusted(
        &policy,
        "srv",
        "url",
        "https://does-not-exist.invalid/mcp",
    )
    .unwrap();
    validate_streamable_http_url_untrusted_dns(
        &policy,
        "srv",
        "url",
        "https://does-not-exist.invalid/mcp",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn argv_placeholder_errors_do_not_leak_plain_argv() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);

    let server_cfg = ServerConfig::stdio(vec![
        "mcp-server-bin".to_string(),
        "--auth=Bearer SECRET_TOKEN-${BAD-NAME}".to_string(),
    ])
    .unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("expand argv placeholder"),
        "expected redacted argv context; err={err:#}"
    );
    assert!(
        !msg.contains("SECRET_TOKEN"),
        "argv secret leaked in error chain; err={err:#}"
    );
}

#[test]
fn url_validation_errors_do_not_leak_plain_url() {
    let policy = UntrustedStreamableHttpPolicy::default();

    let err = validate_streamable_http_url_untrusted(
        &policy,
        "srv",
        "url",
        "https://user:pass@example.com/mcp?token=SECRET_TOKEN",
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("url credentials"),
        "expected url credential error; err={err:#}"
    );
    assert!(
        !msg.contains("SECRET_TOKEN"),
        "url secret leaked in error chain; err={err:#}"
    );
    assert!(
        !msg.contains("user:pass"),
        "url userinfo leaked in error chain; err={err:#}"
    );
}

#[tokio::test]
async fn url_placeholder_errors_do_not_leak_plain_url() {
    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);

    let server_cfg =
        ServerConfig::streamable_http("https://example.com/mcp?token=SECRET_TOKEN_${BAD-NAME}")
            .unwrap();

    let err = manager
        .connect("srv", &server_cfg, absolute_test_cwd())
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("expand url placeholder"),
        "expected redacted url context; err={err:#}"
    );
    assert!(
        !msg.contains("SECRET_TOKEN"),
        "url secret leaked in error chain; err={err:#}"
    );
}

#[tokio::test]
async fn disconnect_and_wait_clears_timeout_counter_entry() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after disconnect");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    let srv = ServerName::parse("srv").unwrap();
    manager
        .server_handler_timeout_counts
        .counter_for(&srv)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(manager.server_handler_timeout_count("srv"), 1);

    let status = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_secs(1),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap();
    assert!(status.is_none());

    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));

    server_task.await.unwrap();
}

#[tokio::test]
async fn disconnect_and_wait_times_out_when_handler_task_hangs() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["jsonrpc"], "2.0");
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["jsonrpc"], "2.0");
        assert_eq!(note_value["method"], "notifications/initialized");

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

    let err = manager
        .disconnect_and_wait(
            "srv",
            Duration::from_millis(20),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap_err();
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

#[tokio::test]
async fn connection_wait_aborts_remaining_handler_tasks_after_first_join_error() {
    let (client_stream, _server_stream) = tokio::io::duplex(256);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let slow_task_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dropped_for_task = Arc::clone(&slow_task_dropped);
    let panic_task = tokio::spawn(async move {
        panic!("boom");
    });
    let slow_task = tokio::spawn(async move {
        struct OnDrop(std::sync::Arc<std::sync::atomic::AtomicBool>);

        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let _on_drop = OnDrop(dropped_for_task);
        std::future::pending::<()>().await;
    });

    let conn = Connection {
        id: next_connection_id(),
        child: None,
        client,
        handler_tasks: vec![panic_task, slow_task],
    };

    let err = conn.wait().await.unwrap_err();
    let err_chain = format!("{err:#}");
    assert!(
        err_chain.contains("server handler task panicked"),
        "unexpected error: {err_chain}"
    );

    tokio::time::timeout(Duration::from_secs(1), async {
        while !slow_task_dropped.load(std::sync::atomic::Ordering::Relaxed) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn connection_drop_aborts_handler_tasks() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

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

        let eof = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap();
        assert!(eof.is_none(), "expected EOF after connection drop");
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

    let conn = manager
        .take_connection("srv")
        .expect("connection should exist");
    drop(conn);

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
async fn connection_drop_reaps_child_best_effort() {
    async fn pid_is_alive(pid: u32) -> bool {
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .await
            .is_ok_and(|status| status.success())
    }

    let (client_stream, _server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg("exec sleep 10");
    let child = cmd.spawn().unwrap();
    let child_id = child.id().expect("child id should exist");
    assert!(pid_is_alive(child_id).await);

    let conn = Connection {
        id: next_connection_id(),
        child: Some(child),
        client,
        handler_tasks: Vec::new(),
    };
    drop(conn);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !pid_is_alive(child_id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping a connection should reap child process in best-effort mode");
}

#[tokio::test]
async fn connection_wait_with_timeout_uses_single_deadline_budget() {
    let (client_stream, _server_stream) = tokio::io::duplex(256);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(client_read, client_write)
        .await
        .unwrap();

    let mut handler_tasks = Vec::new();
    for _ in 0..3 {
        handler_tasks.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }));
    }

    let conn = Connection {
        id: next_connection_id(),
        child: None,
        client,
        handler_tasks,
    };

    let started = tokio::time::Instant::now();
    let err = conn
        .wait_with_timeout(
            Duration::from_millis(100),
            mcp_jsonrpc::WaitOnTimeout::ReturnError,
        )
        .await
        .unwrap_err();
    let err_chain = format!("{err:#}");
    assert!(
        err_chain.contains("wait timed out after"),
        "unexpected error: {err_chain}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(220),
        "wait exceeded global timeout budget: {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn connection_wait_with_timeout_kill_still_kills_detached_child_when_close_stage_times_out() {
    use std::io;
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
        ) -> Poll<io::Result<usize>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.entered.store(true, Ordering::Relaxed);
            Poll::Pending
        }
    }

    let entered = Arc::new(AtomicBool::new(false));
    let writer = BlockingWrite {
        entered: entered.clone(),
    };
    let (client_stream, _server_stream) = tokio::io::duplex(64);
    let (client_read, _client_write) = tokio::io::split(client_stream);
    let client = mcp_jsonrpc::Client::connect_io(client_read, writer)
        .await
        .unwrap();
    let handle = client.handle();
    let write_task = tokio::spawn(async move {
        let _ = handle
            .notify("demo/stuck", Some(serde_json::json!({ "x": 1 })))
            .await;
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::Relaxed) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("write task should enter write lock");

    let child = tokio::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .expect("spawn child");

    let conn = Connection {
        id: next_connection_id(),
        child: Some(child),
        client,
        handler_tasks: Vec::new(),
    };
    let status = match conn
        .wait_with_timeout(
            Duration::from_millis(20),
            mcp_jsonrpc::WaitOnTimeout::Kill {
                kill_timeout: Duration::from_secs(1),
            },
        )
        .await
    {
        Ok(status) => status,
        Err(err) => {
            panic!("wait should kill detached child even when close stage times out: {err:#}")
        }
    };
    assert!(status.is_some(), "detached child should be reaped");

    write_task.abort();
}

#[tokio::test]
async fn take_connection_clears_timeout_counter_entry() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, mut server_write) = tokio::io::split(server_stream);

    let server_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_read).lines();

        let init_line = lines.next_line().await.unwrap().unwrap();
        let init_value: Value = serde_json::from_str(&init_line).unwrap();
        assert_eq!(init_value["method"], "initialize");
        let id = init_value["id"].clone();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION },
        });
        let mut response_line = serde_json::to_string(&response).unwrap();
        response_line.push('\n');
        server_write
            .write_all(response_line.as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let note_line = lines.next_line().await.unwrap().unwrap();
        let note_value: Value = serde_json::from_str(&note_line).unwrap();
        assert_eq!(note_value["method"], "notifications/initialized");

        let eof = lines.next_line().await.unwrap();
        assert!(eof.is_none(), "expected EOF after take_connection wait");
    });

    let mut manager = Manager::new("test-client", "0.0.0", Duration::from_secs(5))
        .with_trust_mode(TrustMode::Trusted);
    manager
        .connect_io("srv", client_read, client_write)
        .await
        .unwrap();

    let srv = ServerName::parse("srv").unwrap();
    manager
        .server_handler_timeout_counts
        .counter_for(&srv)
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(manager.server_handler_timeout_count("srv"), 1);

    let conn = manager
        .take_connection(" srv ")
        .expect("connection should exist");
    assert_eq!(manager.server_handler_timeout_count("srv"), 0);
    assert!(!manager.server_handler_timeout_counts().contains_key("srv"));
    assert!(!manager.is_connected("srv"));

    let status = conn.wait().await.unwrap();
    assert!(status.is_none());

    server_task.await.unwrap();
}
