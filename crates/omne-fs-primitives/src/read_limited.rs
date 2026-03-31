use std::fmt;
use std::io::{self, Read};

#[derive(Debug)]
pub enum ReadUtf8Error {
    Io(io::Error),
    TooLarge { bytes: usize, max_bytes: usize },
    InvalidUtf8(std::string::FromUtf8Error),
}

impl fmt::Display for ReadUtf8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::TooLarge { bytes, max_bytes } => {
                write!(f, "file exceeds size limit ({bytes} > {max_bytes} bytes)")
            }
            Self::InvalidUtf8(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ReadUtf8Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::TooLarge { .. } => None,
            Self::InvalidUtf8(error) => Some(error),
        }
    }
}

pub fn read_to_end_limited<R>(reader: &mut R, max_bytes: usize) -> io::Result<(Vec<u8>, bool)>
where
    R: Read,
{
    read_to_end_limited_with_capacity(reader, max_bytes, 0)
}

pub fn read_to_end_limited_with_capacity<R>(
    reader: &mut R,
    max_bytes: usize,
    initial_capacity: usize,
) -> io::Result<(Vec<u8>, bool)>
where
    R: Read,
{
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(initial_capacity);
    reader.take(limit).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > max_bytes;
    Ok((bytes, truncated))
}

pub fn read_utf8_limited<R>(reader: &mut R, max_bytes: usize) -> Result<String, ReadUtf8Error>
where
    R: Read,
{
    let (bytes, truncated) = read_to_end_limited(reader, max_bytes).map_err(ReadUtf8Error::Io)?;
    if truncated {
        return Err(ReadUtf8Error::TooLarge {
            bytes: bytes.len(),
            max_bytes,
        });
    }

    String::from_utf8(bytes).map_err(ReadUtf8Error::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::{
        ReadUtf8Error, read_to_end_limited, read_to_end_limited_with_capacity, read_utf8_limited,
    };
    use std::io::Cursor;

    #[test]
    fn read_to_end_limited_reads_full_buffer_within_limit() {
        let mut cursor = Cursor::new(b"hello".to_vec());
        let (bytes, truncated) = read_to_end_limited(&mut cursor, 5).expect("read");
        assert_eq!(bytes, b"hello");
        assert!(!truncated);
    }

    #[test]
    fn read_to_end_limited_reports_truncation_with_one_extra_byte() {
        let mut cursor = Cursor::new(b"hello".to_vec());
        let (bytes, truncated) = read_to_end_limited(&mut cursor, 4).expect("read");
        assert_eq!(bytes, b"hello");
        assert!(truncated);
    }

    #[test]
    fn read_to_end_limited_handles_zero_limit() {
        let mut cursor = Cursor::new(b"x".to_vec());
        let (bytes, truncated) = read_to_end_limited(&mut cursor, 0).expect("read");
        assert_eq!(bytes, b"x");
        assert!(truncated);
    }

    #[test]
    fn read_to_end_limited_with_capacity_preserves_initial_capacity() {
        let mut cursor = Cursor::new(b"hello".to_vec());
        let (bytes, truncated) =
            read_to_end_limited_with_capacity(&mut cursor, 5, 16).expect("read");
        assert_eq!(bytes, b"hello");
        assert!(!truncated);
        assert!(bytes.capacity() >= 16);
    }

    #[test]
    fn read_utf8_limited_reads_valid_utf8() {
        let mut cursor = Cursor::new("hello".as_bytes().to_vec());

        let text = read_utf8_limited(&mut cursor, 5).expect("read");

        assert_eq!(text, "hello");
    }

    #[test]
    fn read_utf8_limited_reports_truncation() {
        let mut cursor = Cursor::new("hello".as_bytes().to_vec());

        let error = read_utf8_limited(&mut cursor, 4).expect_err("oversized");

        match error {
            ReadUtf8Error::TooLarge { bytes, max_bytes } => {
                assert_eq!(bytes, 5);
                assert_eq!(max_bytes, 4);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn read_utf8_limited_rejects_invalid_utf8() {
        let mut cursor = Cursor::new(vec![0xFF]);

        let error = read_utf8_limited(&mut cursor, 8).expect_err("invalid utf8");

        assert!(matches!(error, ReadUtf8Error::InvalidUtf8(_)));
    }
}
