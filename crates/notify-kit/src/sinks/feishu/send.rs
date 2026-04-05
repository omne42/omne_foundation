use std::time::{SystemTime, UNIX_EPOCH};

use crate::Event;
use crate::sinks::crypto::hmac_sha256_base64;
use crate::sinks::{BoxFuture, Sink};
use http_kit::{read_json_body_after_http_success, send_reqwest};

use super::FeishuWebhookSink;

impl FeishuWebhookSink {
    fn signed_request_fields(&self) -> crate::Result<(Option<String>, Option<String>)> {
        let Some(secret) = self.secret.as_ref() else {
            return Ok((None, None));
        };

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| anyhow::anyhow!("get unix timestamp: {err}"))?
            .as_secs()
            .to_string();

        let secret = secret.expose_secret();
        let string_to_sign = format!("{timestamp}\n{secret}");
        let sign = hmac_sha256_base64(secret, &string_to_sign)?;

        Ok((Some(timestamp), Some(sign)))
    }

    pub(super) fn ensure_success_response(body: &serde_json::Value) -> crate::Result<()> {
        let Some(code) = body["StatusCode"]
            .as_i64()
            .or_else(|| body["code"].as_i64())
        else {
            return Err(crate::error::tagged_message(
                crate::ErrorKind::InvalidResponse,
                "feishu api error: missing status code (response body omitted)",
            )
            .into());
        };

        if code == 0 {
            return Ok(());
        }

        Err(crate::error::tagged_message(
            crate::ErrorKind::Other,
            format!("feishu api error: code={code} (response body omitted)"),
        )
        .into())
    }
}

impl Sink for FeishuWebhookSink {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = self
                .http
                .select_for_url(&self.webhook_url, self.enforce_public_ip)
                .await?;
            let (timestamp, sign) = self.signed_request_fields()?;

            let payload = self
                .build_payload(event, timestamp.as_deref(), sign.as_deref())
                .await?;

            let resp = send_reqwest(
                client.post(self.webhook_url.as_str()).json(&payload),
                "feishu webhook",
            )
            .await?;

            let body = read_json_body_after_http_success(resp, "feishu webhook").await?;
            Self::ensure_success_response(&body)
        })
    }
}
