use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::Semaphore;

use crate::error::{self, ErrorKind};
use crate::public_ip::validate_public_addrs;
use crate::tokio_time;

const DEFAULT_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT: usize = 32;
#[derive(Debug, Clone, PartialEq, Eq)]
struct PinnedRedirectOrigin {
    host: String,
    scheme: String,
    port: u16,
}

impl PinnedRedirectOrigin {
    fn from_url(url: &reqwest::Url) -> crate::Result<Self> {
        let host = url.host_str().ok_or_else(|| {
            error::tagged_message(ErrorKind::InvalidInput, "url must have a host")
        })?;
        let port = url.port_or_known_default().ok_or_else(|| {
            error::tagged_message(
                ErrorKind::InvalidInput,
                "url must have an explicit or known default port",
            )
        })?;
        Ok(Self {
            host: host.to_string(),
            scheme: url.scheme().to_string(),
            port,
        })
    }
}

struct HttpClientSharedState {
    dns_lookup_semaphore: Arc<Semaphore>,
}

impl Default for HttpClientSharedState {
    fn default() -> Self {
        Self {
            dns_lookup_semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT)),
        }
    }
}

impl HttpClientSharedState {
    fn dns_lookup_semaphore(&self) -> &Arc<Semaphore> {
        &self.dns_lookup_semaphore
    }
}

#[derive(Debug, Clone)]
pub struct HttpClientOptions {
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub default_headers: reqwest::header::HeaderMap,
    pub follow_redirects: bool,
    pub no_proxy: bool,
}

impl Default for HttpClientOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            connect_timeout: None,
            default_headers: reqwest::header::HeaderMap::new(),
            follow_redirects: false,
            no_proxy: false,
        }
    }
}

#[derive(Clone)]
pub struct HttpClientProfile {
    client: reqwest::Client,
    options: HttpClientOptions,
    shared_state: Arc<HttpClientSharedState>,
}

impl HttpClientProfile {
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    #[must_use]
    pub fn options(&self) -> &HttpClientOptions {
        &self.options
    }

    pub async fn select_for_url(
        &self,
        url: &reqwest::Url,
        enforce_public_ip: bool,
    ) -> crate::Result<reqwest::Client> {
        select_http_client_from_profile(self, url, enforce_public_ip).await
    }
}

fn dns_lookup_timeout_message() -> String {
    format!("dns lookup timeout (capped at {DEFAULT_DNS_LOOKUP_TIMEOUT:?})")
}

fn dns_lookup_time_driver_message() -> &'static str {
    "dns lookup timeout requires a Tokio runtime with the time driver enabled"
}

fn remaining_dns_timeout(deadline: Instant) -> crate::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining == Duration::ZERO {
        return Err(error::tagged_message(
            ErrorKind::Transport,
            dns_lookup_timeout_message(),
        ));
    }
    Ok(remaining)
}

fn redirect_policy(follow_redirects: bool) -> reqwest::redirect::Policy {
    if follow_redirects {
        reqwest::redirect::Policy::limited(10)
    } else {
        reqwest::redirect::Policy::none()
    }
}

fn pinned_redirect_policy(url: &reqwest::Url, follow_redirects: bool) -> reqwest::redirect::Policy {
    if !follow_redirects {
        return reqwest::redirect::Policy::none();
    }

    let Ok(initial_origin) = PinnedRedirectOrigin::from_url(url) else {
        return reqwest::redirect::Policy::none();
    };

    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }

        let Ok(next_origin) = PinnedRedirectOrigin::from_url(attempt.url()) else {
            return attempt.error("redirect target origin invalid");
        };

        if !next_origin.host.eq_ignore_ascii_case(&initial_origin.host) {
            return attempt.error("redirect target host changed under public-ip pinning");
        }

        if !next_origin
            .scheme
            .eq_ignore_ascii_case(&initial_origin.scheme)
        {
            return attempt.error("redirect target scheme changed under public-ip pinning");
        }

        if next_origin.port != initial_origin.port {
            return attempt.error("redirect target port changed under public-ip pinning");
        }

        attempt.follow()
    })
}

fn build_http_client_builder_with_policy(
    options: &HttpClientOptions,
    redirect_policy: reqwest::redirect::Policy,
    disable_env_proxy: bool,
) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .redirect(redirect_policy)
        .default_headers(options.default_headers.clone());

    if disable_env_proxy || options.no_proxy {
        builder = builder.no_proxy();
    }
    if let Some(timeout) = options.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect_timeout) = options.connect_timeout {
        builder = builder.connect_timeout(connect_timeout);
    }

    builder
}

fn build_http_client_builder(options: &HttpClientOptions) -> reqwest::ClientBuilder {
    build_http_client_builder_with_policy(options, redirect_policy(options.follow_redirects), false)
}

pub fn build_http_client(timeout: Duration) -> crate::Result<reqwest::Client> {
    build_http_client_with_options(&HttpClientOptions {
        timeout: Some(timeout),
        ..Default::default()
    })
}

pub fn parse_header_map_from_str_pairs<I, K, V>(pairs: I) -> crate::Result<HeaderMap>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut out = HeaderMap::new();
    for (name, value) in pairs {
        let name = name.as_ref().trim();
        if name.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
            error::tagged_source(
                ErrorKind::InvalidInput,
                format!("invalid http header name `{name}`"),
                err,
            )
        })?;
        let header_value = HeaderValue::from_str(value.as_ref()).map_err(|err| {
            error::tagged_source(
                ErrorKind::InvalidInput,
                format!("invalid http header value for `{name}`"),
                err,
            )
        })?;
        out.insert(header_name, header_value);
    }

    Ok(out)
}

pub fn build_http_client_with_options(
    options: &HttpClientOptions,
) -> crate::Result<reqwest::Client> {
    build_http_client_builder(options)
        .build()
        .map_err(|err| error::tagged_source(ErrorKind::InvalidInput, "build reqwest client", err))
}

pub fn build_http_client_profile(options: &HttpClientOptions) -> crate::Result<HttpClientProfile> {
    Ok(HttpClientProfile {
        client: build_http_client_with_options(options)?,
        options: options.clone(),
        shared_state: Arc::new(HttpClientSharedState::default()),
    })
}

pub(crate) fn sanitize_reqwest_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_request() {
        "request"
    } else if err.is_decode() {
        "decode"
    } else {
        "unknown"
    }
}

pub async fn send_reqwest(
    builder: reqwest::RequestBuilder,
    context: &str,
) -> crate::Result<reqwest::Response> {
    builder.send().await.map_err(|err| {
        error::tagged_source(
            ErrorKind::Transport,
            format!(
                "{context} request failed ({})",
                sanitize_reqwest_error(&err)
            ),
            err,
        )
    })
}

async fn resolve_url_to_public_addrs_async(
    shared_state: &HttpClientSharedState,
    url: &reqwest::Url,
    timeout: Duration,
) -> crate::Result<Vec<SocketAddr>> {
    let Some(host) = url.host_str() else {
        return Err(error::tagged_message(
            ErrorKind::InvalidInput,
            "url must have a host",
        ));
    };

    let port = url.port_or_known_default().ok_or_else(|| {
        error::tagged_message(
            ErrorKind::InvalidInput,
            "url must have an explicit or known default port",
        )
    })?;
    let dns_timeout = timeout.min(DEFAULT_DNS_LOOKUP_TIMEOUT);
    if dns_timeout == Duration::ZERO {
        return Err(error::tagged_message(
            ErrorKind::Transport,
            dns_lookup_timeout_message(),
        ));
    }

    let deadline = Instant::now() + dns_timeout;
    let lookup = {
        let _permit = tokio_time::timeout(
            remaining_dns_timeout(deadline)?,
            shared_state.dns_lookup_semaphore().acquire(),
        )
        .await
        .map_err(|_| error::tagged_message(ErrorKind::Transport, dns_lookup_time_driver_message()))?
        .map_err(|err| {
            error::tagged_source(ErrorKind::Transport, dns_lookup_timeout_message(), err)
        })?
        .map_err(|err| error::tagged_source(ErrorKind::Transport, "dns lookup failed", err))?;

        tokio_time::timeout(
            remaining_dns_timeout(deadline)?,
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| error::tagged_message(ErrorKind::Transport, dns_lookup_time_driver_message()))?
        .map_err(|err| {
            error::tagged_source(ErrorKind::Transport, dns_lookup_timeout_message(), err)
        })?
        .map_err(|err| error::tagged_source(ErrorKind::Transport, "dns lookup failed", err))?
    };

    validate_public_addrs(lookup)
}

fn resolve_override_addrs_for_reqwest(url: &reqwest::Url, addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    if url.port().is_some() {
        return addrs.to_vec();
    }

    addrs
        .iter()
        .copied()
        .map(|mut addr| {
            addr.set_port(0);
            addr
        })
        .collect()
}

fn dns_lookup_timeout_for_options(options: &HttpClientOptions) -> Duration {
    match (options.timeout, options.connect_timeout) {
        (Some(timeout), Some(connect_timeout)) => timeout.min(connect_timeout),
        (Some(timeout), None) => timeout,
        (None, Some(connect_timeout)) => connect_timeout,
        (None, None) => DEFAULT_DNS_LOOKUP_TIMEOUT,
    }
}

fn build_http_client_pinned_with_addrs(
    options: &HttpClientOptions,
    url: &reqwest::Url,
    addrs: &[SocketAddr],
) -> crate::Result<reqwest::Client> {
    let host = url
        .host_str()
        .ok_or_else(|| error::tagged_message(ErrorKind::InvalidInput, "url must have a host"))?;
    let addrs = resolve_override_addrs_for_reqwest(url, addrs);
    build_http_client_builder_with_policy(
        options,
        pinned_redirect_policy(url, options.follow_redirects),
        true,
    )
    .resolve_to_addrs(host, &addrs)
    .build()
    .map_err(|err| error::tagged_source(ErrorKind::InvalidInput, "build reqwest client", err))
}

async fn build_http_client_pinned_async(
    shared_state: &HttpClientSharedState,
    options: &HttpClientOptions,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    let lookup_timeout = dns_lookup_timeout_for_options(options);
    let addrs = resolve_url_to_public_addrs_async(shared_state, url, lookup_timeout).await?;
    build_http_client_pinned_with_addrs(options, url, &addrs)
}

async fn select_pinned_http_client_with_options(
    shared_state: Arc<HttpClientSharedState>,
    options: &HttpClientOptions,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    build_http_client_pinned_async(shared_state.as_ref(), options, url).await
}

pub async fn select_http_client_from_profile(
    profile: &HttpClientProfile,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> crate::Result<reqwest::Client> {
    if !enforce_public_ip {
        return Ok(profile.client.clone());
    }

    // The pinned path disables proxy resolution so the actual socket still targets the
    // DNS-validated address set instead of an intermediate proxy endpoint.
    select_pinned_http_client_with_options(Arc::clone(&profile.shared_state), &profile.options, url)
        .await
}

pub async fn select_http_client_with_options(
    options: &HttpClientOptions,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> crate::Result<reqwest::Client> {
    if !enforce_public_ip {
        // `reqwest::Client` keeps its builder state opaque, so the only way to preserve the
        // documented options contract on the unpinned path is to rebuild from `options`.
        return build_http_client_with_options(options);
    }

    // `reqwest::Client` does not expose a safe way to clone its opaque builder state while
    // swapping in per-host DNS pinning. Callers that need the same configuration on both paths
    // should prefer `HttpClientProfile`, which keeps the reusable options explicit.
    select_pinned_http_client_with_options(Arc::new(HttpClientSharedState::default()), options, url)
        .await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn timeout_only_options(timeout: Duration) -> HttpClientOptions {
        HttpClientOptions {
            timeout: Some(timeout),
            ..Default::default()
        }
    }

    #[test]
    fn parse_header_map_from_str_pairs_accepts_valid_and_skips_empty_names() {
        let map = parse_header_map_from_str_pairs([
            (" x-test ", "value"),
            ("", "ignored"),
            ("   ", "ignored"),
            ("x-other", "123"),
        ])
        .expect("parse headers");
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("x-test")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default(),
            "value"
        );
    }

    #[test]
    fn parse_header_map_from_str_pairs_accepts_borrowed_string_map() {
        let headers = BTreeMap::from([
            ("x-test".to_string(), "value".to_string()),
            ("x-other".to_string(), "123".to_string()),
        ]);
        let map = parse_header_map_from_str_pairs(headers.iter()).expect("parse headers");
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("x-other")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default(),
            "123"
        );
    }

    #[test]
    fn parse_header_map_from_str_pairs_rejects_invalid_header_name() {
        let err = parse_header_map_from_str_pairs([("bad header", "value")])
            .expect_err("invalid header name should fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("invalid http header name"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn parse_header_map_from_str_pairs_rejects_invalid_header_value() {
        let err = parse_header_map_from_str_pairs([("x-test", "bad\nvalue")])
            .expect_err("invalid header value should fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("invalid http header value"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn dns_lookup_timeout_prefers_connect_timeout() {
        let timeout = dns_lookup_timeout_for_options(&HttpClientOptions {
            timeout: Some(Duration::from_secs(30)),
            connect_timeout: Some(Duration::from_millis(250)),
            ..Default::default()
        });
        assert_eq!(timeout, Duration::from_millis(250));

        let timeout = dns_lookup_timeout_for_options(&HttpClientOptions {
            timeout: Some(Duration::from_millis(500)),
            connect_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        assert_eq!(timeout, Duration::from_millis(500));
    }

    #[test]
    fn dns_lookup_timeout_falls_back_to_request_timeout_then_default() {
        let timeout = dns_lookup_timeout_for_options(&HttpClientOptions {
            timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        });
        assert_eq!(timeout, Duration::from_secs(3));

        let default_timeout = dns_lookup_timeout_for_options(&HttpClientOptions::default());
        assert_eq!(default_timeout, DEFAULT_DNS_LOOKUP_TIMEOUT);
    }

    #[test]
    fn remaining_dns_timeout_accepts_future_deadline() {
        let remaining =
            remaining_dns_timeout(Instant::now() + Duration::from_millis(10)).expect("timeout");
        assert!(remaining > Duration::ZERO);
        assert!(remaining <= Duration::from_millis(10));
    }

    #[test]
    fn remaining_dns_timeout_rejects_elapsed_deadline() {
        let err =
            remaining_dns_timeout(Instant::now()).expect_err("elapsed deadline should be rejected");
        assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");
    }

    #[test]
    fn resolve_override_addrs_uses_scheme_default_port_when_url_has_no_explicit_port() {
        let url = reqwest::Url::parse("http://example.com/webhook").expect("parse url");
        let addrs =
            resolve_override_addrs_for_reqwest(&url, &[SocketAddr::from(([203, 0, 113, 10], 80))]);
        assert_eq!(addrs, vec![SocketAddr::from(([203, 0, 113, 10], 0))]);
    }

    #[test]
    fn build_http_client_pinned_with_addrs_preserves_default_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0_u8; 2048];
            let read = stream.read(&mut buf).expect("read request");
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(
                request.contains("x-test-header: pinned\r\n"),
                "request should keep default headers: {request}"
            );

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("pinned"),
            );
            let options = HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                default_headers,
                ..Default::default()
            };
            let url = reqwest::Url::parse(&format!("http://example.test:{}/hook", addr.port()))
                .expect("parse url");
            let client = build_http_client_pinned_with_addrs(&options, &url, &[addr])
                .expect("build pinned client");

            let response = client.get(url).send().await.expect("send request");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn build_http_client_profile_preserves_default_headers_for_base_client() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0_u8; 2048];
            let read = stream.read(&mut buf).expect("read request");
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(
                request.contains("x-test-header: profile\r\n"),
                "request should keep default headers: {request}"
            );

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("profile"),
            );
            let profile = build_http_client_profile(&HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                default_headers,
                ..Default::default()
            })
            .expect("build http client profile");
            let url = reqwest::Url::parse(&format!("http://127.0.0.1:{}/hook", addr.port()))
                .expect("parse url");

            let response = profile
                .select_for_url(&url, false)
                .await
                .expect("select base client")
                .get(url)
                .send()
                .await
                .expect("send request");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn select_http_client_with_options_rebuilds_unpinned_client_from_options() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0_u8; 2048];
            let read = stream.read(&mut buf).expect("read request");
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(
                request.contains("x-test-header: options\r\n"),
                "request should rebuild the unpinned client from explicit options: {request}"
            );

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("options"),
            );
            let options = HttpClientOptions {
                timeout: Some(Duration::from_secs(1)),
                default_headers,
                ..Default::default()
            };
            let url = reqwest::Url::parse(&format!("http://127.0.0.1:{}/hook", addr.port()))
                .expect("parse url");

            let response = select_http_client_with_options(&options, &url, false)
                .await
                .expect("select unpinned client from options")
                .get(url)
                .send()
                .await
                .expect("send request");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_rejects_cross_host_redirects() {
        let redirect_listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect listener");
        let redirect_addr = redirect_listener
            .local_addr()
            .expect("redirect listener addr");
        let blocked_listener = TcpListener::bind("127.0.0.1:0").expect("bind blocked listener");
        let blocked_addr = blocked_listener
            .local_addr()
            .expect("blocked listener addr");

        let redirect_server = thread::spawn(move || {
            let (mut stream, _) = redirect_listener.accept().expect("accept redirect request");
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                blocked_addr.port()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write redirect response");
        });

        let blocked_server = thread::spawn(move || {
            blocked_listener
                .set_nonblocking(true)
                .expect("set nonblocking");
            let deadline = Instant::now() + Duration::from_millis(300);
            loop {
                match blocked_listener.accept() {
                    Ok(_) => panic!("redirect target should not be contacted"),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("accept failed: {err}"),
                }
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let url = reqwest::Url::parse(&format!(
                "http://public.example:{}/redirect",
                redirect_addr.port()
            ))
            .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[redirect_addr],
            )
            .expect("build pinned client");

            let err = client
                .get(url)
                .send()
                .await
                .expect_err("cross-host redirect should fail");
            assert!(
                err.is_redirect() || err.is_request(),
                "unexpected redirect error classification: {err}"
            );
        });

        redirect_server.join().expect("join redirect server");
        blocked_server.join().expect("join blocked server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_rejects_same_host_scheme_redirects() {
        let redirect_listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect listener");
        let redirect_addr = redirect_listener
            .local_addr()
            .expect("redirect listener addr");

        let redirect_server = thread::spawn(move || {
            let (mut stream, _) = redirect_listener.accept().expect("accept redirect request");
            let response = b"HTTP/1.1 302 Found\r\nLocation: https://public.example/secure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(response).expect("write redirect response");
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let url = reqwest::Url::parse(&format!(
                "http://public.example:{}/redirect",
                redirect_addr.port()
            ))
            .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[redirect_addr],
            )
            .expect("build pinned client");

            let err = client
                .get(url)
                .send()
                .await
                .expect_err("same-host scheme redirect should fail");
            assert!(
                err.is_redirect() || err.is_request(),
                "unexpected redirect error classification: {err}"
            );
        });

        redirect_server.join().expect("join redirect server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_rejects_same_host_port_redirects() {
        let redirect_listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect listener");
        let redirect_addr = redirect_listener
            .local_addr()
            .expect("redirect listener addr");
        let blocked_listener = TcpListener::bind("127.0.0.1:0").expect("bind blocked listener");
        let blocked_addr = blocked_listener
            .local_addr()
            .expect("blocked listener addr");

        let redirect_server = thread::spawn(move || {
            let (mut stream, _) = redirect_listener.accept().expect("accept redirect request");
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://public.example:{}/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                blocked_addr.port()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write redirect response");
        });

        let blocked_server = thread::spawn(move || {
            blocked_listener
                .set_nonblocking(true)
                .expect("set nonblocking");
            let deadline = Instant::now() + Duration::from_millis(300);
            loop {
                match blocked_listener.accept() {
                    Ok(_) => panic!("redirect target should not be contacted"),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("accept failed: {err}"),
                }
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let url = reqwest::Url::parse(&format!(
                "http://public.example:{}/redirect",
                redirect_addr.port()
            ))
            .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[redirect_addr],
            )
            .expect("build pinned client");

            let err = client
                .get(url)
                .send()
                .await
                .expect_err("same-host port redirect should fail");
            assert!(
                err.is_redirect() || err.is_request(),
                "unexpected redirect error classification: {err}"
            );
        });

        redirect_server.join().expect("join redirect server");
        blocked_server.join().expect("join blocked server");
    }

    #[test]
    fn build_http_client_pinned_with_addrs_allows_same_host_redirects_and_keeps_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");

        let server = thread::spawn(move || {
            for expected_path in ["/redirect", "/final"] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut buf = [0_u8; 2048];
                let read = stream.read(&mut buf).expect("read request");
                let request = String::from_utf8_lossy(&buf[..read]);

                assert!(
                    request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")),
                    "unexpected request line: {request}"
                );
                assert!(
                    request.contains("x-test-header: pinned\r\n"),
                    "request should keep default headers across same-host redirects: {request}"
                );

                let response = if expected_path == "/redirect" {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://public.example:{}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        addr.port()
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .to_string()
                };
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut default_headers = reqwest::header::HeaderMap::new();
            default_headers.insert(
                "x-test-header",
                reqwest::header::HeaderValue::from_static("pinned"),
            );
            let url =
                reqwest::Url::parse(&format!("http://public.example:{}/redirect", addr.port()))
                    .expect("parse url");
            let client = build_http_client_pinned_with_addrs(
                &HttpClientOptions {
                    timeout: Some(Duration::from_secs(1)),
                    default_headers,
                    follow_redirects: true,
                    ..Default::default()
                },
                &url,
                &[addr],
            )
            .expect("build pinned client");

            let response = client.get(url).send().await.expect("same-host redirect");
            assert!(response.status().is_success());
        });

        server.join().expect("join server");
    }

    #[test]
    fn select_http_client_from_profile_re_resolves_dns_on_every_pinned_selection() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::ZERO;
            let url = reqwest::Url::parse("https://future-cache-bypass.invalid/webhook")
                .expect("parse url");

            let profile =
                build_http_client_profile(&timeout_only_options(timeout)).expect("build profile");
            let err = profile
                .select_for_url(&url, true)
                .await
                .expect_err("expected dns timeout error");
            assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");
        });
    }

    #[test]
    fn build_http_client_profile_uses_explicit_per_profile_shared_state() {
        let profile_a = build_http_client_profile(&timeout_only_options(Duration::from_secs(1)))
            .expect("build profile a");
        let profile_b = build_http_client_profile(&timeout_only_options(Duration::from_secs(1)))
            .expect("build profile b");
        let profile_a_clone = profile_a.clone();

        assert!(
            Arc::ptr_eq(&profile_a.shared_state, &profile_a_clone.shared_state),
            "cloned profile should keep the same explicit shared state"
        );
        assert!(
            !Arc::ptr_eq(&profile_a.shared_state, &profile_b.shared_state),
            "distinct profiles should not share process-global pinned-client state"
        );
    }

    #[test]
    fn select_http_client_with_public_ip_pinning_returns_error_without_time_driver() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let url = reqwest::Url::parse("https://example.com/mcp").expect("parse url");
            let err = select_http_client_with_options(
                &timeout_only_options(Duration::from_secs(1)),
                &url,
                true,
            )
            .await
            .expect_err("missing time driver should return an error");

            assert_eq!(err.kind(), ErrorKind::Transport);
            assert!(
                err.to_string().contains("time driver enabled"),
                "unexpected error: {err}"
            );
        });
    }
}
