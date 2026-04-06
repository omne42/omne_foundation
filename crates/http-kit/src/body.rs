use std::io::Write;

use crate::error::{self, ErrorKind};
use crate::{client::sanitize_reqwest_error, url::redact_url_for_error};

pub const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024;
const RESPONSE_BODY_DRAIN_LIMIT_BYTES: usize = 64 * 1024;
const ERROR_RESPONSE_SUMMARY_MAX_CHARS: usize = 200;

pub async fn read_json_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<serde_json::Value> {
    let buf = read_body_bytes_limited(resp, max_bytes).await?;
    serde_json::from_slice(&buf)
        .map_err(|err| error::tagged_source(ErrorKind::ResponseDecode, "decode json failed", err))
}

pub async fn read_text_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<String> {
    let (buf, truncated) = read_body_bytes_truncated(resp, max_bytes).await?;
    Ok(decode_text_body_lossy(buf, truncated))
}

pub async fn write_response_body_limited<W>(
    mut resp: reqwest::Response,
    writer: &mut W,
    max_bytes: Option<u64>,
) -> crate::Result<()>
where
    W: Write + ?Sized,
{
    let url = redact_url_for_error(resp.url());
    if let (Some(limit), Some(content_length)) = (max_bytes, resp.content_length()) {
        ensure_response_body_size_within_limit(content_length, limit, &url)?;
    }

    let mut downloaded_bytes = 0_u64;
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        error::tagged_source(
            ErrorKind::ResponseBody,
            format!(
                "read response body failed ({})",
                sanitize_reqwest_error(&err)
            ),
            err,
        )
    })? {
        downloaded_bytes = downloaded_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| {
                error::tagged_message(ErrorKind::ResponseBody, "response body size overflow")
            })?;
        if let Some(limit) = max_bytes {
            ensure_response_body_size_within_limit(downloaded_bytes, limit, &url)?;
        }
        writer.write_all(&chunk).map_err(|err| {
            error::tagged_source(ErrorKind::ResponseBody, "write response body failed", err)
        })?;
    }

    Ok(())
}

pub fn response_body_read_error(
    label: &str,
    status: reqwest::StatusCode,
    err: &crate::Error,
) -> crate::Error {
    error::tagged_message(
        ErrorKind::ResponseBody,
        format!("{label}: {status} (failed to read response body: {err})"),
    )
}

pub fn http_status_text_error(
    context: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> crate::Error {
    let summary = truncate_chars(body.trim(), ERROR_RESPONSE_SUMMARY_MAX_CHARS);
    if summary.is_empty() {
        return error::tagged_message(
            ErrorKind::HttpStatus,
            format!("{context} http error: {status} (response body omitted)"),
        );
    }

    error::tagged_message(
        ErrorKind::HttpStatus,
        format!("{context} http error: {status}, response={summary}"),
    )
}

pub async fn ensure_http_success(resp: reqwest::Response, context: &str) -> crate::Result<()> {
    let status = resp.status();
    if status.is_success() {
        try_drain_response_body_for_reuse(resp).await;
        return Ok(());
    }

    let body = read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES)
        .await
        .map_err(|err| response_body_read_error(&format!("{context} http error"), status, &err))?;
    Err(http_status_text_error(context, status, &body))
}

pub async fn read_json_body_after_http_success(
    resp: reqwest::Response,
    context: &str,
) -> crate::Result<serde_json::Value> {
    read_json_body_after_http_success_limited(resp, context, DEFAULT_MAX_RESPONSE_BODY_BYTES).await
}

pub async fn read_json_body_after_http_success_limited(
    resp: reqwest::Response,
    context: &str,
    max_bytes: usize,
) -> crate::Result<serde_json::Value> {
    let status = resp.status();
    if !status.is_success() {
        let body = read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES)
            .await
            .map_err(|err| {
                response_body_read_error(&format!("{context} http error"), status, &err)
            })?;
        return Err(http_status_text_error(context, status, &body));
    }

    read_json_body_limited(resp, max_bytes).await
}

pub async fn drain_response_body(mut resp: reqwest::Response) {
    while let Ok(Some(_chunk)) = resp.chunk().await {}
}

pub async fn read_response_body_preview_text(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }

    let mut out = Vec::with_capacity(max_bytes.min(4096));
    while let Ok(Some(chunk)) = resp.chunk().await {
        let remaining = max_bytes.saturating_sub(out.len());
        if remaining == 0 {
            break;
        }

        let take = remaining.min(chunk.len());
        out.extend_from_slice(&chunk[..take]);
        if out.len() == max_bytes {
            break;
        }
    }

    body_preview_text(&out, max_bytes)
}

pub fn body_preview_json(body: &[u8], max_bytes: usize) -> Option<serde_json::Value> {
    body_preview_text(body, max_bytes).map(|preview| serde_json::json!({ "body": preview }))
}

pub fn body_preview_text(body: &[u8], max_bytes: usize) -> Option<String> {
    if max_bytes == 0 || body.is_empty() {
        return None;
    }

    let preview_len = body.len().min(max_bytes);
    let preview = String::from_utf8_lossy(&body[..preview_len]).into_owned();
    Some(truncate_string_to_bytes(preview, max_bytes))
}

async fn try_drain_response_body_for_reuse(mut resp: reqwest::Response) {
    if matches!(resp.content_length(), Some(0)) {
        return;
    }
    drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
}

fn decode_text_body_lossy(buf: Vec<u8>, truncated: bool) -> String {
    let mut out = match String::from_utf8(buf) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(&err.into_bytes()).into_owned(),
    };
    if truncated {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("[truncated]");
    }
    out
}

async fn read_body_bytes_limited(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<Vec<u8>> {
    if max_bytes == 0 {
        drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
        return Err(error::tagged_message(
            ErrorKind::ResponseBody,
            "response body too large (response body omitted)",
        ));
    }

    let mut cap_hint = 0usize;
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
            return Err(error::tagged_message(
                ErrorKind::ResponseBody,
                "response body too large (response body omitted)",
            ));
        }
        cap_hint = content_length_capacity_hint(len, max_bytes);
    }

    let mut buf = Vec::with_capacity(cap_hint);
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        error::tagged_source(
            ErrorKind::ResponseBody,
            format!(
                "read response body failed ({})",
                sanitize_reqwest_error(&err)
            ),
            err,
        )
    })? {
        if chunk.len() > max_bytes.saturating_sub(buf.len()) {
            drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
            return Err(error::tagged_message(
                ErrorKind::ResponseBody,
                "response body too large (response body omitted)",
            ));
        }
        buf.extend_from_slice(&chunk);
    }

    Ok(buf)
}

async fn read_body_bytes_truncated(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<(Vec<u8>, bool)> {
    if max_bytes == 0 {
        drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
        return Ok((Vec::new(), true));
    }

    let mut truncated = false;
    let mut cap_hint = 0usize;
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            truncated = true;
        }
        cap_hint = content_length_capacity_hint(len, max_bytes);
    }

    let mut buf = Vec::with_capacity(cap_hint);
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        error::tagged_source(
            ErrorKind::ResponseBody,
            format!(
                "read response body failed ({})",
                sanitize_reqwest_error(&err)
            ),
            err,
        )
    })? {
        if buf.len() >= max_bytes {
            truncated = true;
            break;
        }

        let remaining = max_bytes - buf.len();
        if chunk.len() > remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }

        buf.extend_from_slice(&chunk);
    }

    if truncated {
        drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
    }

    Ok((buf, truncated))
}

async fn drain_response_body_limited(resp: &mut reqwest::Response, mut remaining: usize) {
    while remaining > 0 {
        let Ok(Some(chunk)) = resp.chunk().await else {
            break;
        };
        remaining = remaining.saturating_sub(chunk.len());
    }
}

fn content_length_capacity_hint(content_length: u64, max_bytes: usize) -> usize {
    usize::try_from(content_length)
        .ok()
        .map_or(max_bytes, |len| len.min(max_bytes))
}

fn ensure_response_body_size_within_limit(size: u64, limit: u64, url: &str) -> crate::Result<()> {
    if size > limit {
        return Err(error::tagged_message(
            ErrorKind::ResponseBody,
            format!(
                "response body size {size} exceeds configured max download size {limit} for {url}"
            ),
        ));
    }

    Ok(())
}

fn truncate_string_to_bytes(mut s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    s.truncate(end);
    s
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let total_chars = input.chars().count();
    if total_chars <= max_chars {
        return input.to_string();
    }

    if max_chars <= 3 {
        return input.chars().take(max_chars).collect();
    }

    let prefix: String = input.chars().take(max_chars - 3).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_text_body_lossy_reuses_valid_utf8_buffer() {
        let bytes = b"ok".to_vec();
        let ptr = bytes.as_ptr();
        let out = decode_text_body_lossy(bytes, false);
        assert_eq!(out, "ok");
        assert_eq!(out.as_ptr(), ptr);
    }

    #[test]
    fn decode_text_body_lossy_handles_invalid_utf8() {
        let out = decode_text_body_lossy(vec![0xff, b'a'], false);
        assert_eq!(out, "\u{fffd}a");
    }

    #[test]
    fn decode_text_body_lossy_marks_truncated_output() {
        let out = decode_text_body_lossy(b"line".to_vec(), true);
        assert_eq!(out, "line\n[truncated]");
    }

    #[test]
    fn http_status_text_error_uses_omitted_for_empty_body() {
        let err =
            http_status_text_error("discord webhook", reqwest::StatusCode::BAD_REQUEST, "   ");
        assert_eq!(
            err.to_string(),
            "discord webhook http error: 400 Bad Request (response body omitted)"
        );
    }

    #[test]
    fn http_status_text_error_truncates_body_summary() {
        let long_body = "x".repeat(300);
        let err = http_status_text_error(
            "generic webhook",
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            &long_body,
        );
        let msg = err.to_string();
        assert!(
            msg.starts_with("generic webhook http error: 500 Internal Server Error, response="),
            "{msg}"
        );
        assert!(msg.ends_with("..."), "{msg}");
    }

    #[test]
    fn body_preview_text_is_bounded_by_max_bytes() {
        let body = b"abcdefghijklmnopqrstuvwxyz";
        let preview = body_preview_text(body, 8).expect("preview available");
        assert_eq!(preview, "abcdefgh");
    }

    #[test]
    fn body_preview_json_returns_none_for_zero_limit() {
        let body = b"{\"large\":true}";
        assert!(body_preview_json(body, 0).is_none());
    }

    #[test]
    fn ensure_response_body_size_within_limit_reports_redacted_url() {
        let err = ensure_response_body_size_within_limit(10, 8, "https://example.com/")
            .expect_err("expected size limit error");
        assert_eq!(
            err.to_string(),
            "response body size 10 exceeds configured max download size 8 for https://example.com/"
        );
    }

    #[test]
    fn read_json_body_after_http_success_returns_json_on_success() {
        let listener = match std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)) {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping read_json_body_after_http_success_returns_json_on_success: loopback bind not permitted in this environment: {err}"
                );
                return;
            }
            Err(err) => panic!("bind listener: {err}"),
        };
        let addr = listener.local_addr().expect("listener addr");

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/json\r\n",
                "Content-Length: 11\r\n",
                "Connection: close\r\n",
                "\r\n",
                "{\"ok\":true}"
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let client = reqwest::Client::builder().build().expect("build client");
            let resp = client
                .get(format!("http://{addr}/"))
                .send()
                .await
                .expect("send request");

            let body = read_json_body_after_http_success(resp, "test api")
                .await
                .expect("json body");
            assert_eq!(body["ok"].as_bool(), Some(true));
        });

        server.join().expect("server thread");
    }

    #[test]
    fn write_response_body_limited_streams_response_to_writer() {
        let listener = match std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)) {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping write_response_body_limited_streams_response_to_writer: loopback bind not permitted in this environment: {err}"
                );
                return;
            }
            Err(err) => panic!("bind listener: {err}"),
        };
        let addr = listener.local_addr().expect("listener addr");

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/octet-stream\r\n",
                "Content-Length: 5\r\n",
                "Connection: close\r\n",
                "\r\n",
                "hello"
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let client = reqwest::Client::builder().build().expect("build client");
            let resp = client
                .get(format!("http://{addr}/"))
                .send()
                .await
                .expect("send request");

            let mut out = Vec::new();
            write_response_body_limited(resp, &mut out, Some(8))
                .await
                .expect("write body");
            assert_eq!(out, b"hello");
        });

        server.join().expect("server thread");
    }
}
