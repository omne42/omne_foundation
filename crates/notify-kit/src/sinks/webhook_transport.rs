use std::time::Duration;

use http_kit::{HttpClientOptions, HttpClientProfile, build_http_client_profile};

#[derive(Clone)]
pub(crate) struct WebhookTransport {
    profile: HttpClientProfile,
    default_enforce_public_ip: bool,
}

impl WebhookTransport {
    pub(crate) fn new(timeout: Duration, enforce_public_ip: bool) -> crate::Result<Self> {
        Ok(Self {
            profile: build_http_client_profile(&HttpClientOptions {
                timeout: Some(timeout),
                ..Default::default()
            })?,
            default_enforce_public_ip: enforce_public_ip,
        })
    }

    pub(crate) async fn client_for(&self, url: &reqwest::Url) -> crate::Result<reqwest::Client> {
        self.client_for_with_public_ip(url, self.default_enforce_public_ip)
            .await
    }

    pub(crate) async fn client_for_with_public_ip(
        &self,
        url: &reqwest::Url,
        enforce_public_ip: bool,
    ) -> crate::Result<reqwest::Client> {
        Ok(self.profile.select_for_url(url, enforce_public_ip).await?)
    }

    pub(crate) async fn validate_public_ip(&self, url: &reqwest::Url) -> crate::Result<()> {
        self.client_for_with_public_ip(url, true).await.map(|_| ())
    }

    pub(crate) fn default_enforce_public_ip(&self) -> bool {
        self.default_enforce_public_ip
    }

    #[cfg(test)]
    pub(crate) fn set_default_enforce_public_ip(&mut self, enforce_public_ip: bool) {
        self.default_enforce_public_ip = enforce_public_ip;
    }
}
