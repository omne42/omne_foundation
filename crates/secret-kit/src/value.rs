use std::fmt::{self, Formatter};
use std::sync::Arc;

use zeroize::Zeroize;

use crate::{Result, SecretError};

/// Heap-backed secret text that redacts itself in `Debug` output and zeroizes its shared buffer
/// when the last handle drops.
#[derive(Clone, Default)]
pub struct SecretString(Arc<SecretText>);

#[derive(Default)]
struct SecretText(String);

impl SecretText {
    fn into_inner(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::new(SecretText(value.into())))
    }

    /// Borrow the plaintext secret.
    ///
    /// This returns a normal `&str`. If the caller copies it into another `String`, logs it, or
    /// stores it elsewhere, that external copy is outside `SecretString`'s zeroization contract.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.0.as_str()
    }

    /// Extract the owned secret string without cloning.
    ///
    /// This only succeeds when the current handle uniquely owns the underlying buffer.
    pub fn into_inner(self) -> std::result::Result<String, Self> {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(shared) => Err(Self(shared)),
        }
    }

    /// Consume the secret and return owned plaintext.
    ///
    /// This reuses the underlying allocation when the current handle is unique and clones only
    /// when the secret buffer is shared, such as after cache hits.
    ///
    /// The returned `String` is ordinary owned plaintext. Once it leaves `SecretString`, the
    /// caller is responsible for its lifetime and any further scrubbing.
    #[must_use]
    pub fn into_owned(self) -> String {
        match self.into_inner() {
            Ok(value) => value,
            Err(shared) => shared.expose_secret().to_owned(),
        }
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl Drop for SecretText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Raw secret bytes that zeroize their current buffer on drop.
#[derive(Default)]
pub(crate) struct SecretBytes(pub(crate) Vec<u8>);

impl SecretBytes {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    pub(crate) fn into_inner(mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

impl AsRef<[u8]> for SecretBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes(<redacted>, len={})", self.0.len())
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

struct ZeroizingByteBuffer<const N: usize>([u8; N]);

impl<const N: usize> ZeroizingByteBuffer<N> {
    fn new() -> Self {
        Self([0u8; N])
    }
}

impl<const N: usize> AsRef<[u8]> for ZeroizingByteBuffer<N> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> AsMut<[u8]> for ZeroizingByteBuffer<N> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl<const N: usize> Drop for ZeroizingByteBuffer<N> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub(crate) async fn read_limited<R>(
    mut reader: R,
    max_bytes: usize,
) -> std::io::Result<(SecretBytes, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut out = SecretBytes::with_capacity(max_bytes);
    let mut buf = ZeroizingByteBuffer::<4096>::new();
    let mut truncated = false;
    loop {
        let remaining = max_bytes.saturating_sub(out.len());
        let read_len = buf.as_ref().len().min(remaining.saturating_add(1).max(1));
        let n = reader.read(&mut buf.as_mut()[..read_len]).await?;
        if n == 0 {
            break;
        }

        if n > remaining {
            out.extend_from_slice(&buf.as_ref()[..remaining]);
            truncated = true;
            break;
        }

        out.extend_from_slice(&buf.as_ref()[..n]);
    }
    Ok((out, truncated))
}

pub(crate) fn secret_string_from_bytes(
    bytes: SecretBytes,
    invalid_utf8_error: impl FnOnce(std::str::Utf8Error) -> SecretError,
) -> Result<SecretString> {
    match String::from_utf8(bytes.into_inner()) {
        Ok(value) => Ok(SecretString::from(value)),
        Err(err) => {
            let utf8_error = err.utf8_error();
            let mut bytes = err.into_bytes();
            bytes.zeroize();
            Err(invalid_utf8_error(utf8_error))
        }
    }
}
