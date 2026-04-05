mod config;
mod constructor;
mod media;
mod payload;
mod send;

pub use config::FeishuWebhookConfig;
pub use constructor::FeishuWebhookSink;
use constructor::TenantAccessTokenState;

const FEISHU_MAX_CHARS: usize = 4000;
const FEISHU_DEFAULT_IMAGE_UPLOAD_MAX_BYTES: usize = 10 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    #[cfg(unix)]
    use std::{os::unix::fs::symlink, os::unix::net::UnixListener};

    use super::*;
    use crate::sinks::feishu::media::LoadedImage;
    use crate::{Event, SecretString};

    fn local_image_test_root() -> PathBuf {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("notify-kit-feishu-tests");
        std::fs::create_dir_all(&base).expect("create local image test root");
        base
    }

    fn unique_local_image_test_name(label: &str) -> String {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("unix epoch")
            .as_nanos();
        format!("{label}-{}-{unique:x}", std::process::id())
    }

    #[cfg(unix)]
    fn unix_socket_test_root() -> PathBuf {
        let candidate = PathBuf::from("/dev/shm");
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_dir() => candidate,
            _ => local_image_test_root(),
        }
    }

    #[cfg(unix)]
    fn unique_unix_socket_test_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("unix epoch")
            .as_nanos();
        unix_socket_test_root().join(format!(
            "nk-{}-{:08x}.sock",
            std::process::id(),
            unique as u32
        ))
    }

    fn local_image_enabled_config() -> FeishuWebhookConfig {
        let root = local_image_test_root().join(unique_local_image_test_name("local-root"));
        std::fs::create_dir_all(&root).expect("create local image root");
        FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_app_credentials("app_id", "app_secret")
            .with_local_image_files(true)
            .with_local_image_root(root)
    }

    #[test]
    fn builds_expected_text_payload() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = FeishuWebhookSink::build_text_payload(&event, FEISHU_MAX_CHARS, None, None);
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "text");
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn builds_post_payload_for_markdown_body() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("hello [lark](https://open.feishu.cn)\n\n![img](https://example.com/a.png)")
            .with_tag("thread_id", "t1");

        let sink = FeishuWebhookSink::new(FeishuWebhookConfig::new(
            "https://open.feishu.cn/open-apis/bot/v2/hook/x",
        ))
        .expect("build sink");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let payload = rt
            .block_on(sink.build_payload(&event, None, None))
            .expect("build payload");
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "post");

        let content = payload["content"]["post"]["zh_cn"]["content"]
            .as_array()
            .expect("array content");
        assert!(!content.is_empty());

        let text_payload = payload.to_string();
        assert!(text_payload.contains("\"tag\":\"a\""), "{text_payload}");
        assert!(text_payload.contains("thread_id=t1"), "{text_payload}");
        assert!(text_payload.contains("[image:img]"), "{text_payload}");
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = FeishuWebhookConfig::new("http://open.feishu.cn/open-apis/bot/v2/hook/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = FeishuWebhookConfig::new("https://example.com/open-apis/bot/v2/hook/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_path() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/api/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn strict_requires_public_ip_check() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_public_ip_check(false);
        let err = FeishuWebhookSink::new_strict(cfg).expect_err("expected strict validation");
        assert!(err.to_string().contains("public ip"), "{err:#}");
        assert_eq!(err.kind(), crate::ErrorKind::Config);
    }

    #[test]
    fn explicit_public_ip_validation_requires_public_ip_check() {
        let sink = FeishuWebhookSink::new(
            FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                .with_public_ip_check(false),
        )
        .expect("build sink");
        let err = sink
            .validate_public_ip_sync()
            .expect_err("expected explicit validation failure");
        assert!(
            err.to_string().contains("with_public_ip_check(true)"),
            "{err:#}"
        );
        assert_eq!(err.kind(), crate::ErrorKind::Config);
    }

    #[test]
    fn strict_sync_constructor_rejects_inside_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x");
            let err =
                FeishuWebhookSink::new_strict(cfg).expect_err("expected runtime constructor error");
            assert!(err.to_string().contains("validate_public_ip"), "{err:#}");
            assert_eq!(err.kind(), crate::ErrorKind::Config);
        });
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let url = "https://open.feishu.cn/open-apis/bot/v2/hook/secret_token";
        let cfg = FeishuWebhookConfig::new(url);
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret_token"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("open.feishu.cn"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = FeishuWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret_token"), "{sink_dbg}");
        assert!(sink_dbg.contains("open.feishu.cn"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn builds_payload_with_signature_fields() {
        let event = Event::new("kind", crate::Severity::Info, "title");
        let payload = FeishuWebhookSink::build_text_payload(
            &event,
            FEISHU_MAX_CHARS,
            Some("123"),
            Some("sig"),
        );
        assert_eq!(payload["timestamp"].as_str().unwrap_or(""), "123");
        assert_eq!(payload["sign"].as_str().unwrap_or(""), "sig");
    }

    #[test]
    fn trims_secret() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x");
        let sink =
            FeishuWebhookSink::new_with_secret(cfg, "  my_secret  ").expect("build secret sink");
        assert_eq!(
            sink.secret.as_ref().map(SecretString::expose_secret),
            Some("my_secret")
        );
    }

    #[test]
    fn payload_respects_max_chars() {
        let event = Event::new("kind", crate::Severity::Info, "title").with_body("x".repeat(100));
        let payload = FeishuWebhookSink::build_text_payload(&event, 10, None, None);
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.chars().count() <= 10, "{text}");
        assert!(text.ends_with("..."), "{text}");
    }

    #[test]
    fn normalizes_app_credentials() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_app_credentials("  app_id  ", "  app_secret  ");
        let sink = FeishuWebhookSink::new(cfg).expect("build sink");
        let creds = sink.app_credentials.expect("credentials");
        assert_eq!(creds.app_id, "app_id");
        assert_eq!(creds.app_secret.expose_secret(), "app_secret");
    }

    #[test]
    fn local_image_files_are_disabled_by_default() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");

            let err = sink
                .load_image("./should-not-be-read.png")
                .await
                .expect_err("local files should be disabled");
            assert!(
                err.to_string().contains("local image files are disabled"),
                "{err:#}"
            );
        });
    }

    #[test]
    fn remote_image_urls_are_disabled_by_default() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");

            let err = sink
                .load_image("https://example.com/image.png")
                .await
                .expect_err("remote image urls should be disabled");
            assert!(
                err.to_string().contains("remote image urls are disabled"),
                "{err:#}"
            );
        });
    }

    #[test]
    fn local_image_opt_in_requires_explicit_roots() {
        let err = FeishuWebhookSink::new(
            FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                .with_app_credentials("app_id", "app_secret")
                .with_local_image_files(true),
        )
        .expect_err("local image opt-in without roots should fail closed");
        assert!(
            err.to_string().contains("configured local image root"),
            "{err:#}"
        );
    }

    #[test]
    fn remote_images_follow_public_ip_check_setting() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_public_ip_check(false)
                    .with_remote_image_urls(true),
            )
            .expect("build sink");

            let err = sink
                .load_image("https://localhost/image.png")
                .await
                .expect_err("remote image host validation should still reject localhost");
            assert!(err.to_string().contains("host is not allowed"), "{err:#}");
        });
    }

    #[test]
    fn local_image_files_require_explicit_opt_in() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let config = local_image_enabled_config();
            let root = config.local_image_roots.first().expect("configured root");
            let path = root.join(format!(
                "{}.png",
                unique_local_image_test_name("notify-kit-feishu-local-image")
            ));
            std::fs::write(&path, b"png").expect("write local image");

            let sink = FeishuWebhookSink::new(config).expect("build sink");

            let loaded = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect("load local image");
            assert_eq!(loaded.bytes, b"png");
            assert_eq!(loaded.content_type, "image/png");

            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn local_image_files_reject_paths_outside_configured_roots() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let allowed_root =
                local_image_test_root().join(unique_local_image_test_name("allowed"));
            let outside_path = local_image_test_root().join(format!(
                "{}-outside.png",
                unique_local_image_test_name("outside")
            ));
            std::fs::create_dir_all(&allowed_root).expect("create allowed root");
            std::fs::write(&outside_path, b"png").expect("write outside image");

            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_local_image_files(true)
                    .with_local_image_root(allowed_root.clone()),
            )
            .expect("build sink");

            let err = sink
                .load_image(outside_path.to_str().expect("utf8 path"))
                .await
                .expect_err("paths outside configured roots should be rejected");
            assert!(
                err.to_string()
                    .contains("outside configured local image roots"),
                "{err:#}"
            );

            let _ = std::fs::remove_dir_all(allowed_root);
            let _ = std::fs::remove_file(outside_path);
        });
    }

    #[test]
    fn local_image_files_reject_non_regular_paths() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let config = local_image_enabled_config();
            let root = config.local_image_roots.first().expect("configured root");
            let path = root.join(unique_local_image_test_name(
                "notify-kit-feishu-local-image-dir",
            ));
            std::fs::create_dir_all(&path).expect("create local image dir");

            let sink = FeishuWebhookSink::new(config).expect("build sink");

            let err = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect_err("directories should be rejected");
            assert!(err.to_string().contains("regular file"), "{err:#}");

            let _ = std::fs::remove_dir_all(path);
        });
    }

    #[cfg(unix)]
    #[test]
    fn local_image_files_reject_symlinks() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let config = local_image_enabled_config();
            let root = config
                .local_image_roots
                .first()
                .expect("configured root")
                .clone();
            let name = unique_local_image_test_name("notify-kit-feishu-local-image");
            let target = root.join(format!("{name}-target.png"));
            let link = root.join(format!("{name}-link.png"));
            std::fs::write(&target, b"png").expect("write symlink target");
            symlink(&target, &link).expect("create symlink");

            let sink = FeishuWebhookSink::new(config).expect("build sink");

            let err = sink
                .load_image(link.to_str().expect("utf8 path"))
                .await
                .expect_err("symlinks should be rejected");
            assert!(err.to_string().contains("symlink component"), "{err:#}");

            let _ = std::fs::remove_file(link);
            let _ = std::fs::remove_file(target);
        });
    }

    #[cfg(unix)]
    #[test]
    fn local_image_files_reject_symlink_ancestors() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let base = local_image_test_root().join(unique_local_image_test_name(
                "notify-kit-feishu-local-image-ancestor",
            ));
            let target_dir = base.join("target");
            let link_dir = base.join("link");
            let image_name = "image.png";
            let target = target_dir.join(image_name);
            std::fs::create_dir_all(&target_dir).expect("create target dir");
            std::fs::write(&target, b"png").expect("write image");
            symlink(&target_dir, &link_dir).expect("create dir symlink");

            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_local_image_files(true)
                    .with_local_image_root(base.clone()),
            )
            .expect("build sink");

            let err = sink
                .load_image(link_dir.join(image_name).to_str().expect("utf8 path"))
                .await
                .expect_err("symlink ancestors should be rejected");
            assert!(err.to_string().contains("symlink component"), "{err:#}");

            let _ = std::fs::remove_file(&target);
            let _ = std::fs::remove_file(&link_dir);
            let _ = std::fs::remove_dir_all(base);
        });
    }

    #[cfg(unix)]
    #[test]
    fn local_image_files_reject_unix_socket_paths() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let path = unique_unix_socket_test_path();
            let listener = UnixListener::bind(&path).expect("create unix socket");

            let sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret")
                    .with_local_image_files(true)
                    .with_local_image_root(path.parent().expect("socket parent").to_path_buf()),
            )
            .expect("build sink");

            let err = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect_err("unix socket paths should be rejected");
            assert!(err.to_string().contains("regular file"), "{err:#}");

            drop(listener);
            let _ = std::fs::remove_file(path);
        });
    }

    #[cfg(not(unix))]
    #[test]
    fn local_image_files_fail_closed_on_platforms_without_safe_open() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let path = local_image_test_root().join(format!(
                "{}.png",
                unique_local_image_test_name("notify-kit-feishu-local-image-unsupported")
            ));
            std::fs::write(&path, b"png").expect("write local image");

            let sink = FeishuWebhookSink::new(local_image_enabled_config()).expect("build sink");

            let err = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect_err("platform should fail closed");
            assert!(
                err.to_string().contains("not supported on this platform"),
                "{err:#}"
            );

            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn local_image_files_reject_oversized_files_before_upload() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let config = local_image_enabled_config().with_image_upload_max_bytes(4);
            let root = config
                .local_image_roots
                .first()
                .expect("configured root")
                .clone();
            let path = root.join(format!(
                "{}.png",
                unique_local_image_test_name("notify-kit-feishu-local-image-large")
            ));
            std::fs::write(&path, vec![b'x'; 5]).expect("write oversized image");

            let sink = FeishuWebhookSink::new(config).expect("build sink");

            let err = sink
                .load_image(path.to_str().expect("utf8 path"))
                .await
                .expect_err("oversized files should be rejected");
            assert!(err.to_string().contains("too large"), "{err:#}");

            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn concurrent_token_refresh_is_singleflight() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&hits);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            server_hits.fetch_add(1, Ordering::SeqCst);

            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf).expect("read request");

            let body = r#"{"code":0,"tenant_access_token":"token","expires_in":7200}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let mut sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");
            sink.webhook_url =
                reqwest::Url::parse(&format!("http://{addr}/open-apis/bot/v2/hook/x"))
                    .expect("parse local webhook url");
            sink.enforce_public_ip = false;

            let sink = Arc::new(sink);
            let mut tasks = Vec::new();
            for _ in 0..8 {
                let sink = Arc::clone(&sink);
                tasks.push(tokio::spawn(async move {
                    sink.ensure_tenant_access_token().await
                }));
            }

            for task in tasks {
                let token = task
                    .await
                    .expect("join token task")
                    .expect("resolve tenant token");
                assert_eq!(token.expose_secret(), "token");
            }
        });

        server.join().expect("join server");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_token_refresh_resets_state_and_allows_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let (first_hit_tx, first_hit_rx) = tokio::sync::oneshot::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut first_stream, _) = listener.accept().expect("accept first connection");
            let mut buf = [0_u8; 1024];
            let _ = first_stream.read(&mut buf).expect("read first request");
            first_hit_tx.send(()).expect("signal first request");
            release_first_rx.recv().expect("release first request");
            drop(first_stream);

            let (mut second_stream, _) = listener.accept().expect("accept second connection");
            let _ = second_stream.read(&mut buf).expect("read second request");
            let body = r#"{"code":0,"tenant_access_token":"token-after-retry","expires_in":7200}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            second_stream
                .write_all(response.as_bytes())
                .expect("write second response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let mut sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");
            sink.webhook_url =
                reqwest::Url::parse(&format!("http://{addr}/open-apis/bot/v2/hook/x"))
                    .expect("parse local webhook url");
            sink.enforce_public_ip = false;

            let sink = Arc::new(sink);
            let refresh_task = tokio::spawn({
                let sink = Arc::clone(&sink);
                async move { sink.ensure_tenant_access_token().await }
            });

            first_hit_rx.await.expect("wait for first token request");
            refresh_task.abort();
            let _ = refresh_task.await;
            release_first_tx.send(()).expect("release first request");

            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let guard = sink.tenant_access_token.lock().await;
                    if matches!(&*guard, TenantAccessTokenState::Empty) {
                        break;
                    }
                    drop(guard);
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("refresh cancellation should reset token state");

            let token = sink
                .ensure_tenant_access_token()
                .await
                .expect("retry should fetch a fresh token");
            assert_eq!(token.expose_secret(), "token-after-retry");
        });

        server.join().expect("join server");
    }

    #[test]
    fn upload_image_invalidates_cached_token_after_upstream_rejection() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let mut buf = [0_u8; 4096];

            let (mut first_token_stream, _) = listener.accept().expect("accept first token");
            let first_token_read = first_token_stream
                .read(&mut buf)
                .expect("read first token request");
            let first_token_req = String::from_utf8_lossy(&buf[..first_token_read]);
            assert!(
                first_token_req.starts_with("POST /open-apis/auth/v3/tenant_access_token/internal"),
                "{first_token_req}"
            );
            let first_token_body =
                r#"{"code":0,"tenant_access_token":"stale-token","expires_in":7200}"#;
            let first_token_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                first_token_body.len(),
                first_token_body
            );
            first_token_stream
                .write_all(first_token_resp.as_bytes())
                .expect("write first token response");

            let (mut first_upload_stream, _) = listener.accept().expect("accept first upload");
            let first_upload_read = first_upload_stream
                .read(&mut buf)
                .expect("read first upload request");
            let first_upload_req = String::from_utf8_lossy(&buf[..first_upload_read]);
            let first_upload_req_lower = first_upload_req.to_ascii_lowercase();
            assert!(
                first_upload_req.starts_with("POST /open-apis/im/v1/images"),
                "{first_upload_req}"
            );
            assert!(
                first_upload_req_lower.contains("authorization: bearer stale-token"),
                "{first_upload_req}"
            );
            let unauthorized = "token rejected";
            let unauthorized_resp = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                unauthorized.len(),
                unauthorized
            );
            first_upload_stream
                .write_all(unauthorized_resp.as_bytes())
                .expect("write first upload response");

            let (mut second_token_stream, _) = listener.accept().expect("accept second token");
            let second_token_read = second_token_stream
                .read(&mut buf)
                .expect("read second token request");
            let second_token_req = String::from_utf8_lossy(&buf[..second_token_read]);
            assert!(
                second_token_req
                    .starts_with("POST /open-apis/auth/v3/tenant_access_token/internal"),
                "{second_token_req}"
            );
            let second_token_body =
                r#"{"code":0,"tenant_access_token":"fresh-token","expires_in":7200}"#;
            let second_token_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                second_token_body.len(),
                second_token_body
            );
            second_token_stream
                .write_all(second_token_resp.as_bytes())
                .expect("write second token response");

            let (mut second_upload_stream, _) = listener.accept().expect("accept second upload");
            let second_upload_read = second_upload_stream
                .read(&mut buf)
                .expect("read second upload request");
            let second_upload_req = String::from_utf8_lossy(&buf[..second_upload_read]);
            let second_upload_req_lower = second_upload_req.to_ascii_lowercase();
            assert!(
                second_upload_req_lower.contains("authorization: bearer fresh-token"),
                "{second_upload_req}"
            );
            let upload_body = r#"{"code":0,"data":{"image_key":"img-key"}}"#;
            let upload_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                upload_body.len(),
                upload_body
            );
            second_upload_stream
                .write_all(upload_resp.as_bytes())
                .expect("write second upload response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let mut sink = FeishuWebhookSink::new(
                FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
                    .with_app_credentials("app_id", "app_secret"),
            )
            .expect("build sink");
            sink.webhook_url =
                reqwest::Url::parse(&format!("http://{addr}/open-apis/bot/v2/hook/x"))
                    .expect("parse local webhook url");
            sink.enforce_public_ip = false;

            let image = LoadedImage {
                bytes: b"png".to_vec(),
                file_name: "image.png".to_string(),
                content_type: "image/png".to_string(),
            };
            let err = sink
                .upload_image(LoadedImage {
                    bytes: image.bytes.clone(),
                    file_name: image.file_name.clone(),
                    content_type: image.content_type.clone(),
                })
                .await
                .expect_err("first upload should fail");
            assert!(err.to_string().contains("401"), "{err:#}");

            let token_state = sink.tenant_access_token.lock().await;
            assert!(
                matches!(&*token_state, TenantAccessTokenState::Empty),
                "rejected token should be dropped"
            );
            drop(token_state);

            let image_key = sink.upload_image(image).await.expect("retry upload");
            assert_eq!(image_key, "img-key");
        });

        server.join().expect("join server");
    }

    #[test]
    fn response_requires_explicit_success_code() {
        let body = serde_json::json!({});
        let err =
            FeishuWebhookSink::ensure_success_response(&body).expect_err("expected missing code");
        assert!(err.to_string().contains("missing status code"), "{err:#}");
    }

    #[test]
    fn response_accepts_zero_code() {
        let body = serde_json::json!({ "StatusCode": 0 });
        FeishuWebhookSink::ensure_success_response(&body).expect("expected success");

        let body = serde_json::json!({ "code": 0 });
        FeishuWebhookSink::ensure_success_response(&body).expect("expected success");
    }
}
