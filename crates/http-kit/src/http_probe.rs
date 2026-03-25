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
            head_error = Some(err.to_string());
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
                Some(head) => format!("HEAD failed: {head}; GET failed: {err}"),
                None => err.to_string(),
            }),
        },
    }
}

pub async fn probe_http_endpoint(client: &reqwest::Client, url: &str) -> bool {
    probe_http_endpoint_detailed(client, url)
        .await
        .is_reachable()
}
