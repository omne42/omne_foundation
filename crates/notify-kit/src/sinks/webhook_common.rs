use std::time::Duration;

use http_kit::{
    HttpClientOptions, HttpClientProfile, build_http_client_profile, parse_and_validate_https_url,
    send_reqwest, validate_url_path_prefix,
};

pub(super) struct JsonWebhookEndpoint {
    url: reqwest::Url,
    http: HttpClientProfile,
    enforce_public_ip: bool,
}

impl JsonWebhookEndpoint {
    pub(super) fn new_validated_https(
        url: &str,
        allowed_hosts: &[&str],
        path_prefix: &str,
        timeout: Duration,
        enforce_public_ip: bool,
    ) -> crate::Result<Self> {
        let url = parse_and_validate_https_url(url, allowed_hosts)?;
        validate_url_path_prefix(&url, path_prefix)?;
        Self::from_url(url, timeout, enforce_public_ip)
    }

    pub(super) fn from_url(
        url: reqwest::Url,
        timeout: Duration,
        enforce_public_ip: bool,
    ) -> crate::Result<Self> {
        let http = build_http_client_profile(&HttpClientOptions {
            timeout: Some(timeout),
            ..Default::default()
        })?;
        Ok(Self {
            url,
            http,
            enforce_public_ip,
        })
    }

    pub(super) fn url(&self) -> &reqwest::Url {
        &self.url
    }

    pub(super) fn url_mut(&mut self) -> &mut reqwest::Url {
        &mut self.url
    }

    pub(super) fn enforce_public_ip(&self) -> bool {
        self.enforce_public_ip
    }

    pub(super) async fn post_json(
        &self,
        payload: &serde_json::Value,
        sink_name: &'static str,
    ) -> crate::Result<reqwest::Response> {
        self.post_json_to(&self.url, payload, sink_name).await
    }

    pub(super) async fn post_json_to(
        &self,
        url: &reqwest::Url,
        payload: &serde_json::Value,
        sink_name: &'static str,
    ) -> crate::Result<reqwest::Response> {
        let client = self
            .http
            .select_for_url(url, self.enforce_public_ip)
            .await?;
        Ok(send_reqwest(client.post(url.as_str()).json(payload), sink_name).await?)
    }
}
