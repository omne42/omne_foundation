#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use reqwest::Url;
use reqwest::header::{HeaderName, HeaderValue};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum HttpAuthError {
    #[error("{field} must not be empty")]
    FieldRequired { field: &'static str },
    #[error("OAuth token response is missing access_token")]
    OAuthResponseMissingAccessToken,
    #[error("OAuth token request failed: {0}")]
    OAuthRequest(#[from] reqwest::Error),
    #[error("invalid HTTP auth header name {header}: {message}")]
    HeaderNameInvalid { header: String, message: String },
    #[error("invalid HTTP auth header value for {header}: {message}")]
    HeaderValueInvalid { header: String, message: String },
    #[error("failed to format SigV4 amz date: {0}")]
    SigV4FormatAmzDate(time::error::Format),
    #[error("failed to format SigV4 date: {0}")]
    SigV4FormatDate(time::error::Format),
    #[error("SigV4 amz date is too short")]
    SigV4AmzDateTooShort,
    #[error("SigV4 method must not be empty")]
    SigV4MethodEmpty,
    #[error("invalid SigV4 URL {url}: {message}")]
    SigV4UrlInvalid { url: String, message: String },
    #[error("SigV4 URL is missing host")]
    SigV4UrlMissingHost,
    #[error("invalid SigV4 HMAC key: {0}")]
    SigV4HmacKeyInvalid(String),
}

pub type Result<T> = std::result::Result<T, HttpAuthError>;

#[derive(Clone)]
pub struct HttpHeaderAuth {
    pub header: HeaderName,
    pub value: HeaderValue,
}

impl std::fmt::Debug for HttpHeaderAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpHeaderAuth")
            .field("header", &self.header)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl HttpHeaderAuth {
    pub fn bearer(token: &str) -> Result<Self> {
        Self::header_value("authorization", Some("Bearer "), token)
    }

    pub fn header_value(header: &str, prefix: Option<&str>, token: &str) -> Result<Self> {
        let header = header.trim();
        ensure_not_empty("header", header)?;

        let header_name = HeaderName::from_bytes(header.as_bytes()).map_err(|err| {
            HttpAuthError::HeaderNameInvalid {
                header: header.to_string(),
                message: err.to_string(),
            }
        })?;

        let mut out = String::new();
        if let Some(prefix) = prefix {
            out.push_str(prefix);
        }
        out.push_str(token);
        let mut value =
            HeaderValue::from_str(&out).map_err(|err| HttpAuthError::HeaderValueInvalid {
                header: header_name.as_str().to_string(),
                message: err.to_string(),
            })?;
        value.set_sensitive(true);

        Ok(Self {
            header: header_name,
            value,
        })
    }

    #[must_use]
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header(self.header.clone(), self.value.clone())
    }

    #[must_use]
    pub fn as_header_pair(&self) -> (&HeaderName, &HeaderValue) {
        (&self.header, &self.value)
    }
}

#[derive(Clone)]
pub struct HttpQueryParamAuth {
    pub param: String,
    pub value: String,
}

impl std::fmt::Debug for HttpQueryParamAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpQueryParamAuth")
            .field("param", &self.param)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl HttpQueryParamAuth {
    pub fn new(param: &str, prefix: Option<&str>, token: &str) -> Result<Self> {
        let param = param.trim();
        ensure_not_empty("param", param)?;

        let mut value = String::new();
        if let Some(prefix) = prefix {
            value.push_str(prefix);
        }
        value.push_str(token);

        Ok(Self {
            param: param.to_string(),
            value,
        })
    }

    #[must_use]
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.query(&[(self.param.as_str(), self.value.as_str())])
    }

    #[must_use]
    pub fn as_query_pair(&self) -> (&str, &str) {
        (self.param.as_str(), self.value.as_str())
    }
}

#[derive(Clone, Debug)]
pub enum HttpRequestAuth {
    Http(HttpHeaderAuth),
    QueryParam(HttpQueryParamAuth),
}

impl HttpRequestAuth {
    #[must_use]
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Http(auth) => auth.apply(req),
            Self::QueryParam(auth) => auth.apply(req),
        }
    }

    #[must_use]
    pub fn as_header_pair(&self) -> Option<(&HeaderName, &HeaderValue)> {
        match self {
            Self::Http(auth) => Some(auth.as_header_pair()),
            Self::QueryParam(_) => None,
        }
    }

    #[must_use]
    pub fn as_query_pair(&self) -> Option<(&str, &str)> {
        match self {
            Self::Http(_) => None,
            Self::QueryParam(auth) => Some(auth.as_query_pair()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HttpRequestAuthPlan {
    pub auth: Option<HttpRequestAuth>,
    pub query_params: BTreeMap<String, String>,
}

impl HttpRequestAuthPlan {
    #[must_use]
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = match self.auth.as_ref() {
            Some(auth) => auth.apply(req),
            None => req,
        };
        apply_query_params(req, &self.query_params)
    }
}

#[must_use]
pub fn apply_query_params<K, V, I>(
    mut req: reqwest::RequestBuilder,
    params: I,
) -> reqwest::RequestBuilder
where
    K: AsRef<str>,
    V: AsRef<str>,
    I: IntoIterator<Item = (K, V)>,
{
    for (name, value) in params {
        let name = name.as_ref().trim();
        if name.is_empty() {
            continue;
        }
        req = req.query(&[(name, value.as_ref())]);
    }
    req
}

#[derive(Clone)]
pub struct OAuthToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub scope: Option<String>,
}

impl std::fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

impl OAuthToken {
    #[must_use]
    pub fn authorization_header_value(&self) -> String {
        format!("{} {}", self.token_type, self.access_token)
    }
}

#[derive(Clone)]
pub struct OAuthClientCredentials {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    pub audience: Option<String>,
    pub extra_params: BTreeMap<String, String>,
}

impl std::fmt::Debug for OAuthClientCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let extra_param_keys: Vec<&str> = self.extra_params.keys().map(String::as_str).collect();
        f.debug_struct("OAuthClientCredentials")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.scope)
            .field("audience", &self.audience)
            .field("extra_params", &extra_param_keys)
            .finish()
    }
}

impl OAuthClientCredentials {
    pub fn new(
        token_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self> {
        let token_url = token_url.into();
        let client_id = client_id.into();
        let client_secret = client_secret.into();

        ensure_not_empty("token_url", &token_url)?;
        ensure_not_empty("client_id", &client_id)?;
        ensure_not_empty("client_secret", &client_secret)?;

        Ok(Self {
            token_url,
            client_id,
            client_secret,
            scope: None,
            audience: None,
            extra_params: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    #[must_use]
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    #[must_use]
    pub fn with_extra_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_params.insert(key.into(), value.into());
        self
    }

    pub async fn fetch_token(&self, http: &reqwest::Client) -> Result<OAuthToken> {
        let mut params = Vec::<(String, String)>::new();
        params.push(("grant_type".to_string(), "client_credentials".to_string()));
        params.push(("client_id".to_string(), self.client_id.clone()));
        params.push(("client_secret".to_string(), self.client_secret.clone()));
        if let Some(scope) = self.scope.as_ref().filter(|s| !s.trim().is_empty()) {
            params.push(("scope".to_string(), scope.clone()));
        }
        if let Some(audience) = self.audience.as_ref().filter(|s| !s.trim().is_empty()) {
            params.push(("audience".to_string(), audience.clone()));
        }
        for (key, value) in &self.extra_params {
            if !key.trim().is_empty() {
                params.push((key.clone(), value.clone()));
            }
        }

        let parsed = http
            .post(self.token_url.as_str())
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;
        let access_token = parsed
            .access_token
            .filter(|token| !token.trim().is_empty())
            .ok_or(HttpAuthError::OAuthResponseMissingAccessToken)?;
        let token_type = parsed
            .token_type
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Bearer".to_string());

        Ok(OAuthToken {
            access_token,
            token_type,
            expires_in: parsed.expires_in,
            scope: parsed.scope,
        })
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SigV4Timestamp {
    pub amz_date: String,
    pub date: String,
}

impl SigV4Timestamp {
    pub fn now() -> Result<Self> {
        Self::from_datetime(OffsetDateTime::now_utc())
    }

    pub fn from_datetime(datetime: OffsetDateTime) -> Result<Self> {
        const AMZ_FORMAT: &[FormatItem<'_>] =
            format_description!("[year][month][day]T[hour][minute][second]Z");
        const DATE_FORMAT: &[FormatItem<'_>] = format_description!("[year][month][day]");

        let amz_date = datetime
            .format(AMZ_FORMAT)
            .map_err(HttpAuthError::SigV4FormatAmzDate)?;
        let date = datetime
            .format(DATE_FORMAT)
            .map_err(HttpAuthError::SigV4FormatDate)?;
        Ok(Self { amz_date, date })
    }

    pub fn from_amz_date(amz_date: &str) -> Result<Self> {
        let amz_date = amz_date.trim();
        if amz_date.len() < 8 {
            return Err(HttpAuthError::SigV4AmzDateTooShort);
        }
        Ok(Self {
            amz_date: amz_date.to_string(),
            date: amz_date[..8].to_string(),
        })
    }
}

#[derive(Clone)]
pub struct SigV4Signer {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    region: String,
    service: String,
}

impl std::fmt::Debug for SigV4Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigV4Signer")
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .field("region", &self.region)
            .field("service", &self.service)
            .finish()
    }
}

impl SigV4Signer {
    pub fn new(
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        session_token: Option<String>,
        region: impl Into<String>,
        service: impl Into<String>,
    ) -> Result<Self> {
        let access_key = access_key.into();
        let secret_key = secret_key.into();
        let region = region.into();
        let service = service.into();

        ensure_not_empty("access_key", &access_key)?;
        ensure_not_empty("secret_key", &secret_key)?;
        ensure_not_empty("region", &region)?;
        ensure_not_empty("service", &service)?;

        Ok(Self {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        })
    }

    pub fn sign(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        payload: &[u8],
        timestamp: SigV4Timestamp,
    ) -> Result<SigV4SigningResult> {
        let method = method.trim();
        if method.is_empty() {
            return Err(HttpAuthError::SigV4MethodEmpty);
        }

        let url = Url::parse(url).map_err(|err| HttpAuthError::SigV4UrlInvalid {
            url: url.to_string(),
            message: err.to_string(),
        })?;
        let host = url.host_str().ok_or(HttpAuthError::SigV4UrlMissingHost)?;
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };

        let payload_hash = sha256_hex(payload);
        let canonical_headers_map = prepare_headers(
            headers,
            &host,
            &timestamp.amz_date,
            &payload_hash,
            self.session_token.as_deref(),
        );
        let (canonical_headers, signed_headers) = canonical_headers(&canonical_headers_map);
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            canonical_uri(&url),
            canonical_query(&url),
            canonical_headers,
            signed_headers,
            payload_hash
        );

        let scope = format!(
            "{}/{}/{}/aws4_request",
            timestamp.date, self.region, self.service
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            timestamp.amz_date,
            scope,
            sha256_hex(canonical_request.as_bytes())
        );
        let signature = self.sign_string(&timestamp.date, &string_to_sign)?;
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, scope, signed_headers, signature
        );

        let headers = SigV4Headers {
            authorization,
            amz_date: timestamp.amz_date,
            content_sha256: payload_hash,
            host,
            security_token: self.session_token.clone(),
        };

        Ok(SigV4SigningResult {
            headers,
            signed_headers,
            signature,
            canonical_request,
            string_to_sign,
        })
    }

    fn sign_string(&self, date: &str, string_to_sign: &str) -> Result<String> {
        let k_date = hmac_sha256(format!("AWS4{}", self.secret_key).as_bytes(), date)?;
        let k_region = hmac_sha256(&k_date, self.region.as_str())?;
        let k_service = hmac_sha256(&k_region, self.service.as_str())?;
        let k_signing = hmac_sha256(&k_service, "aws4_request")?;
        let signature = hmac_sha256(&k_signing, string_to_sign)?;
        Ok(hex_encode(&signature))
    }
}

#[derive(Debug, Clone)]
pub struct SigV4Headers {
    pub authorization: String,
    pub amz_date: String,
    pub content_sha256: String,
    pub host: String,
    pub security_token: Option<String>,
}

impl SigV4Headers {
    #[must_use]
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = req
            .header("authorization", &self.authorization)
            .header("x-amz-date", &self.amz_date)
            .header("x-amz-content-sha256", &self.content_sha256)
            .header("host", &self.host);
        if let Some(token) = self.security_token.as_ref() {
            req.header("x-amz-security-token", token)
        } else {
            req
        }
    }
}

#[derive(Debug, Clone)]
pub struct SigV4SigningResult {
    pub headers: SigV4Headers,
    pub signed_headers: String,
    pub signature: String,
    pub canonical_request: String,
    pub string_to_sign: String,
}

fn ensure_not_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(HttpAuthError::FieldRequired { field })
    } else {
        Ok(())
    }
}

fn prepare_headers(
    headers: &BTreeMap<String, String>,
    host: &str,
    amz_date: &str,
    payload_hash: &str,
    session_token: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::<String, String>::new();
    for (name, value) in headers {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let key = name.to_ascii_lowercase();
        let value = normalize_header_value(value);
        if let Some(existing) = out.get_mut(&key) {
            if !existing.is_empty() {
                existing.push(',');
            }
            existing.push_str(&value);
        } else {
            out.insert(key, value);
        }
    }

    out.entry("host".to_string())
        .or_insert_with(|| host.to_string());
    out.insert("x-amz-date".to_string(), amz_date.to_string());
    out.entry("x-amz-content-sha256".to_string())
        .or_insert_with(|| payload_hash.to_string());
    if let Some(token) = session_token {
        out.insert(
            "x-amz-security-token".to_string(),
            normalize_header_value(token),
        );
    }
    out
}

fn canonical_headers(headers: &BTreeMap<String, String>) -> (String, String) {
    let mut canonical_headers = String::new();
    let mut signed_headers = Vec::<String>::new();

    for (name, value) in headers {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value);
        canonical_headers.push('\n');
        signed_headers.push(name.clone());
    }

    (canonical_headers, signed_headers.join(";"))
}

fn canonical_uri(url: &Url) -> String {
    let path = url.path();
    if path.is_empty() {
        "/".to_string()
    } else {
        aws_percent_encode(path, false)
    }
}

fn canonical_query(url: &Url) -> String {
    let mut pairs = Vec::<(String, String)>::new();
    for (name, value) in url.query_pairs() {
        pairs.push((
            aws_percent_encode(&name, true),
            aws_percent_encode(&value, true),
        ));
    }
    pairs.sort();
    pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn aws_percent_encode(value: &str, encode_slash: bool) -> String {
    let mut out = String::new();
    for &byte in value.as_bytes() {
        let is_unreserved =
            matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~');
        if is_unreserved || (!encode_slash && byte == b'/') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX_CHARS[usize::from(byte >> 4)]));
            out.push(char::from(HEX_CHARS[usize::from(byte & 0x0f)]));
        }
    }
    out
}

fn normalize_header_value(value: &str) -> String {
    let mut out = String::new();
    let mut last_space = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

fn hmac_sha256(key: &[u8], data: &str) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|err| HttpAuthError::SigV4HmacKeyInvalid(err.to_string()))?;
    mac.update(data.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex_encode(&digest)
}

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(HEX_CHARS[usize::from(byte >> 4)]));
        out.push(char::from(HEX_CHARS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use httpmock::{Method::POST, MockServer};

    use super::*;

    #[test]
    fn header_auth_builds_sensitive_bearer_header() -> Result<()> {
        let auth = HttpHeaderAuth::bearer("sk-test")?;
        let (name, value) = auth.as_header_pair();

        assert_eq!(name.as_str(), "authorization");
        assert_eq!(value.to_str().unwrap_or_default(), "Bearer sk-test");
        assert!(value.is_sensitive());
        assert!(!format!("{auth:?}").contains("sk-test"));
        Ok(())
    }

    #[test]
    fn header_auth_rejects_invalid_header_name_and_value() {
        assert!(matches!(
            HttpHeaderAuth::header_value("bad header", None, "token"),
            Err(HttpAuthError::HeaderNameInvalid { .. })
        ));
        assert!(matches!(
            HttpHeaderAuth::header_value("authorization", None, "bad\nvalue"),
            Err(HttpAuthError::HeaderValueInvalid { .. })
        ));
    }

    #[test]
    fn query_param_auth_trims_name_and_redacts_debug() -> Result<()> {
        let auth = HttpQueryParamAuth::new(" api_key ", Some("prefix-"), "token")?;
        assert_eq!(auth.as_query_pair(), ("api_key", "prefix-token"));
        assert!(!format!("{auth:?}").contains("token"));
        Ok(())
    }

    #[test]
    fn request_auth_plan_applies_auth_and_query_params() -> Result<()> {
        let auth = HttpRequestAuth::Http(HttpHeaderAuth::bearer("sk-test")?);
        let plan = HttpRequestAuthPlan {
            auth: Some(auth),
            query_params: BTreeMap::from([
                ("".to_string(), "ignored".to_string()),
                ("model".to_string(), "gpt-test".to_string()),
            ]),
        };

        let request = plan
            .apply(reqwest::Client::new().get("https://example.com/v1/models"))
            .build()
            .expect("request builds");
        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer sk-test")
        );
        assert_eq!(request.url().query(), Some("model=gpt-test"));
        Ok(())
    }

    #[tokio::test]
    async fn fetches_oauth_token_via_http() -> Result<()> {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=client_credentials")
                    .body_includes("client_id=test-client")
                    .body_includes("client_secret=secret");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"access_token":"tok-123","token_type":"Bearer","expires_in":3600}"#);
            })
            .await;

        let http = reqwest::Client::new();
        let oauth = OAuthClientCredentials::new(server.url("/token"), "test-client", "secret")?;
        let token = oauth.fetch_token(&http).await?;
        mock.assert_async().await;

        assert_eq!(token.access_token, "tok-123");
        assert_eq!(token.token_type, "Bearer");
        Ok(())
    }

    #[test]
    fn signs_canonical_sigv4_headers() -> Result<()> {
        let signer = SigV4Signer::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            None,
            "us-east-1",
            "iam",
        )?;
        let mut headers = BTreeMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded; charset=utf-8".to_string(),
        );

        let timestamp = SigV4Timestamp::from_amz_date("20150830T123600Z")?;
        let result = signer.sign(
            "GET",
            "https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08",
            &headers,
            b"",
            timestamp,
        )?;

        let expected_canonical = [
            "GET",
            "/",
            "Action=ListUsers&Version=2010-05-08",
            "content-type:application/x-www-form-urlencoded; charset=utf-8",
            "host:iam.amazonaws.com",
            "x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "x-amz-date:20150830T123600Z",
            "",
            "content-type;host;x-amz-content-sha256;x-amz-date",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ]
        .join("\n");

        assert_eq!(result.canonical_request, expected_canonical);
        assert_eq!(
            result.headers.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date, Signature=dd479fa8a80364edf2119ec24bebde66712ee9c9cb2b0d92eb3ab9ccdc0c3947"
        );
        Ok(())
    }
}
