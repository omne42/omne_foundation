use futures_util::TryStreamExt;
use futures_util::stream::{self, BoxStream};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio_util::io::StreamReader;

use crate::error::{self, ErrorKind};

fn sse_limit_error(limit: &str) -> crate::Error {
    error::tagged_message(
        ErrorKind::InvalidInput,
        format!("{limit} must be greater than zero"),
    )
}

fn sse_line_too_large(max_line_bytes: usize) -> crate::Error {
    error::tagged_message(
        ErrorKind::ResponseBody,
        format!("sse line exceeds max_line_bytes {max_line_bytes}"),
    )
}

fn sse_event_too_large(max_event_bytes: usize) -> crate::Error {
    error::tagged_message(
        ErrorKind::ResponseBody,
        format!("sse event exceeds max_event_bytes {max_event_bytes}"),
    )
}

fn sse_read_line_failed(error: impl std::fmt::Display) -> crate::Error {
    error::tagged_message(
        ErrorKind::ResponseBody,
        format!("read sse line failed: {error}"),
    )
}

fn sse_invalid_utf8(error: impl std::fmt::Display) -> crate::Error {
    error::tagged_message(
        ErrorKind::ResponseDecode,
        format!("invalid sse utf-8: {error}"),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseLimits {
    pub max_line_bytes: usize,
    pub max_event_bytes: usize,
}

impl Default for SseLimits {
    fn default() -> Self {
        Self {
            max_line_bytes: 256 * 1024,
            max_event_bytes: 4 * 1024 * 1024,
        }
    }
}

async fn read_next_line_bytes_limited<R>(
    reader: &mut R,
    out: &mut Vec<u8>,
    max_bytes: usize,
) -> crate::Result<bool>
where
    R: AsyncBufRead + Unpin,
{
    if max_bytes == 0 {
        return Err(sse_limit_error("max_line_bytes"));
    }

    out.clear();

    loop {
        let buf = reader.fill_buf().await.map_err(sse_read_line_failed)?;
        if buf.is_empty() {
            return Ok(!out.is_empty());
        }

        let newline_pos = buf.iter().position(|b| *b == b'\n');
        let take_len = newline_pos.map(|pos| pos + 1).unwrap_or(buf.len());

        if out.len().saturating_add(take_len) > max_bytes {
            return Err(sse_line_too_large(max_bytes));
        }

        out.extend_from_slice(&buf[..take_len]);
        reader.consume(take_len);

        if newline_pos.is_some() {
            return Ok(true);
        }
    }
}

async fn read_next_sse_data_with_limits<R>(
    reader: &mut R,
    line_bytes: &mut Vec<u8>,
    buffer: &mut String,
    limits: SseLimits,
) -> crate::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    if limits.max_event_bytes == 0 {
        return Err(sse_limit_error("max_event_bytes"));
    }

    buffer.clear();
    let mut data_field_count = 0usize;

    loop {
        let has_line =
            read_next_line_bytes_limited(reader, line_bytes, limits.max_line_bytes).await?;
        if !has_line {
            if data_field_count == 0 {
                return Ok(None);
            }
            let data = std::mem::take(buffer);
            return Ok(Some(data));
        }

        let line = std::str::from_utf8(line_bytes).map_err(sse_invalid_utf8)?;
        let line = line.trim_end_matches(['\r', '\n']);

        if line.is_empty() {
            if data_field_count == 0 {
                continue;
            }
            let data = std::mem::take(buffer);
            return Ok(Some(data));
        }

        if line.starts_with(':') {
            continue;
        }

        let (field, rest) = match line.split_once(':') {
            Some((field, rest)) => (field, rest.strip_prefix(' ').unwrap_or(rest)),
            None => (line, ""),
        };

        if field == "data" {
            let separator_bytes = usize::from(data_field_count > 0);
            if buffer
                .len()
                .saturating_add(separator_bytes)
                .saturating_add(rest.len())
                > limits.max_event_bytes
            {
                return Err(sse_event_too_large(limits.max_event_bytes));
            }
            if separator_bytes == 1 {
                buffer.push('\n');
            }
            buffer.push_str(rest);
            data_field_count += 1;
        }
    }
}

pub fn sse_data_stream_from_reader_with_limits<R>(
    reader: R,
    limits: SseLimits,
) -> BoxStream<'static, crate::Result<String>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    Box::pin(stream::try_unfold(
        (reader, Vec::<u8>::new(), String::new(), limits),
        |(mut reader, mut line_bytes, mut buffer, limits)| async move {
            match read_next_sse_data_with_limits(&mut reader, &mut line_bytes, &mut buffer, limits)
                .await?
            {
                Some(data) => Ok(Some((data, (reader, line_bytes, buffer, limits)))),
                None => Ok(None),
            }
        },
    ))
}

pub fn sse_data_stream_from_reader<R>(reader: R) -> BoxStream<'static, crate::Result<String>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    sse_data_stream_from_reader_with_limits(reader, SseLimits::default())
}

pub fn sse_data_stream_from_response(
    response: reqwest::Response,
) -> BoxStream<'static, crate::Result<String>> {
    let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(byte_stream);
    sse_data_stream_from_reader(tokio::io::BufReader::new(reader))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use futures_util::stream;
    use tokio_util::bytes::Bytes;

    #[tokio::test]
    async fn parses_sse_data_lines() -> crate::Result<()> {
        let sse = concat!(
            "event: message\n",
            "data: {\"hello\":1}\n\n",
            "data: line1\n",
            "data: line2\n\n",
        );

        let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(sse.to_owned()))]);
        let reader = StreamReader::new(stream);
        let mut out = Vec::new();
        let mut data_stream = sse_data_stream_from_reader(tokio::io::BufReader::new(reader));
        while let Some(item) = data_stream.next().await {
            out.push(item?);
        }

        assert_eq!(out, vec!["{\"hello\":1}", "line1\nline2"]);
        Ok(())
    }

    #[tokio::test]
    async fn preserves_empty_data_events_and_done_literal() -> crate::Result<()> {
        let sse = concat!("data:\n\n", "data: [DONE]\n\n");
        let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(sse.to_owned()))]);
        let reader = StreamReader::new(stream);
        let mut out = Vec::new();
        let mut data_stream = sse_data_stream_from_reader(tokio::io::BufReader::new(reader));
        while let Some(item) = data_stream.next().await {
            out.push(item?);
        }

        assert_eq!(out, vec!["", "[DONE]"]);
        Ok(())
    }

    #[tokio::test]
    async fn preserves_single_optional_space_after_data_colon() -> crate::Result<()> {
        let sse = concat!("data:  indented\n", "data:\tkeeps-tab\n", "data\n\n",);
        let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(sse.to_owned()))]);
        let reader = StreamReader::new(stream);

        let mut data_stream = sse_data_stream_from_reader(tokio::io::BufReader::new(reader));
        let item = data_stream
            .next()
            .await
            .expect("stream item")
            .expect("valid sse item");
        assert_eq!(item, " indented\n\tkeeps-tab\n");
        assert!(data_stream.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn parses_events_across_multiple_stream_chunks() -> crate::Result<()> {
        let stream = stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: hel")),
            Ok(Bytes::from_static(b"lo\n")),
            Ok(Bytes::from_static(b"data: wor")),
            Ok(Bytes::from_static(b"ld\n\n")),
        ]);
        let reader = StreamReader::new(stream);

        let mut data_stream = sse_data_stream_from_reader(tokio::io::BufReader::new(reader));
        let item = data_stream
            .next()
            .await
            .expect("stream item")
            .expect("valid sse item");
        assert_eq!(item, "hello\nworld");
        assert!(data_stream.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_sse_lines_over_max_line_bytes() -> crate::Result<()> {
        let sse = format!("data: {}\n\n", "x".repeat(1024));
        let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(sse))]);
        let reader = StreamReader::new(stream);

        let mut data_stream = sse_data_stream_from_reader_with_limits(
            tokio::io::BufReader::new(reader),
            SseLimits {
                max_line_bytes: 64,
                max_event_bytes: 4096,
            },
        );

        let err = data_stream
            .next()
            .await
            .expect("stream item")
            .expect_err("line too large");
        assert_eq!(err.kind(), ErrorKind::ResponseBody);
        assert_eq!(err.message(), "sse line exceeds max_line_bytes 64");
        Ok(())
    }

    #[tokio::test]
    async fn rejects_zero_max_line_bytes_without_recasting_as_read_failure() -> crate::Result<()> {
        let sse = "data: hello\n\n";
        let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(sse))]);
        let reader = StreamReader::new(stream);

        let mut data_stream = sse_data_stream_from_reader_with_limits(
            tokio::io::BufReader::new(reader),
            SseLimits {
                max_line_bytes: 0,
                max_event_bytes: 128,
            },
        );

        let err = data_stream
            .next()
            .await
            .expect("stream item")
            .expect_err("invalid limit");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(err.message(), "max_line_bytes must be greater than zero");
        Ok(())
    }

    #[tokio::test]
    async fn rejects_sse_events_over_max_event_bytes() -> crate::Result<()> {
        let sse = format!(
            "data: {}\n\ndata: {}\n\n",
            "a".repeat(1024),
            "b".repeat(1024)
        );
        let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(sse))]);
        let reader = StreamReader::new(stream);

        let mut data_stream = sse_data_stream_from_reader_with_limits(
            tokio::io::BufReader::new(reader),
            SseLimits {
                max_line_bytes: 4096,
                max_event_bytes: 128,
            },
        );

        let err = data_stream
            .next()
            .await
            .expect("stream item")
            .expect_err("event too large");
        assert_eq!(err.kind(), ErrorKind::ResponseBody);
        assert!(err.to_string().contains("max_event_bytes 128"));
        Ok(())
    }
}
