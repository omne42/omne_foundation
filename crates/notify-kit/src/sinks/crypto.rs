use base64::Engine as _;
use hmac::Mac as _;

pub(crate) fn hmac_sha256_base64(secret: &str, message: &str) -> crate::Result<String> {
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|err| anyhow::anyhow!("init hmac-sha256: {err}"))?;
    mac.update(message.as_bytes());

    let out = mac.finalize().into_bytes();
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}
