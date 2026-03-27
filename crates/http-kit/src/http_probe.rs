use crate::url::redact_reqwest_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpProbeMethod {
    Head,
    Get,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpProbeKind {
    Reachable,
    HttpError,
    TransportError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProbeResult {
    pub method: HttpProbeMethod,
    pub kind: HttpProbeKind,
    pub status_code: Option<u16>,
    pub detail: Option<String>,
}

impl HttpProbeResult {
    pub fn is_reachable(&self) -> bool {
        self.kind == HttpProbeKind::Reachable
    }
}

pub async fn probe_http_endpoint_detailed(client: &reqwest::Client, url: &str) -> HttpProbeResult {
    let mut head_error = None;
    match client.head(url).send().await {
        Ok(resp) if resp.status().is_success() => {
            return HttpProbeResult {
                method: HttpProbeMethod::Head,
                kind: HttpProbeKind::Reachable,
                status_code: Some(resp.status().as_u16()),
                detail: None,
            };
        }
        Ok(resp) if resp.status() != reqwest::StatusCode::METHOD_NOT_ALLOWED => {
            return HttpProbeResult {
                method: HttpProbeMethod::Head,
                kind: HttpProbeKind::HttpError,
                status_code: Some(resp.status().as_u16()),
                detail: None,
            };
        }
        Ok(_) => {}
        Err(err) => {
            head_error = Some(redact_reqwest_error(&err));
        }
    }

    match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => HttpProbeResult {
            method: HttpProbeMethod::Get,
            kind: HttpProbeKind::Reachable,
            status_code: Some(resp.status().as_u16()),
            detail: None,
        },
        Ok(resp) => HttpProbeResult {
            method: HttpProbeMethod::Get,
            kind: HttpProbeKind::HttpError,
            status_code: Some(resp.status().as_u16()),
            detail: None,
        },
        Err(err) => HttpProbeResult {
            method: HttpProbeMethod::Get,
            kind: HttpProbeKind::TransportError,
            status_code: None,
            detail: Some(match head_error {
                Some(head) => format!(
                    "HEAD failed: {head}; GET failed: {}",
                    redact_reqwest_error(&err)
                ),
                None => redact_reqwest_error(&err),
            }),
        },
    }
}

pub async fn probe_http_endpoint(client: &reqwest::Client, url: &str) -> bool {
    probe_http_endpoint_detailed(client, url)
        .await
        .is_reachable()
}

#[cfg(test)]
mod tests {
    use super::{HttpProbeKind, HttpProbeMethod, probe_http_endpoint_detailed};
    use std::net::TcpListener;
    use std::time::Duration;

    #[tokio::test]
    async fn transport_errors_redact_url_details() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        drop(listener);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .expect("build client");
        let url = format!(
            "http://user:pass@127.0.0.1:{}/secret/path?token=top-secret",
            addr.port()
        );

        let result = probe_http_endpoint_detailed(&client, &url).await;
        assert_eq!(result.method, HttpProbeMethod::Get);
        assert_eq!(result.kind, HttpProbeKind::TransportError);

        let detail = result.detail.expect("transport error detail");
        assert!(detail.contains("127.0.0.1"), "{detail}");
        assert!(!detail.contains("user"), "{detail}");
        assert!(!detail.contains("pass"), "{detail}");
        assert!(!detail.contains("secret/path"), "{detail}");
        assert!(!detail.contains("token=top-secret"), "{detail}");
    }
}
