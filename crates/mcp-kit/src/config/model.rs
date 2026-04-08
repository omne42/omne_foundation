use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use reqwest::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ServerName;
use crate::error::{ErrorKind, tagged_message, wrap_kind};
use crate::protocol::{AUTHORIZATION_HEADER, MCP_PROTOCOL_VERSION_HEADER};

macro_rules! public_bail {
    ($($arg:tt)*) => {
        return Err(tagged_message(ErrorKind::Config, format!($($arg)*)).into())
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Stdio,
    Unix,
    StreamableHttp,
}

#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub protocol_version: Option<String>,
    pub capabilities: Option<Value>,
    pub roots: Option<Vec<Root>>,
}

impl ClientConfig {
    pub fn validate(&self) -> crate::Result<()> {
        if let Some(protocol_version) = self.protocol_version.as_deref() {
            if protocol_version.trim().is_empty() {
                public_bail!("mcp client.protocol_version must not be empty");
            }
        }
        if let Some(capabilities) = self.capabilities.as_ref() {
            if !capabilities.is_object() {
                public_bail!("mcp client.capabilities must be a JSON object");
            }
        }
        if let Some(roots) = self.roots.as_ref() {
            for (idx, root) in roots.iter().enumerate() {
                if root.uri.trim().is_empty() {
                    public_bail!("mcp client.roots[{idx}].uri must not be empty");
                }
                if let Some(name) = root.name.as_deref() {
                    if name.trim().is_empty() {
                        public_bail!("mcp client.roots[{idx}].name must not be empty");
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub uri: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdoutLogConfig {
    pub path: PathBuf,
    pub max_bytes_per_part: u64,
    pub max_parts: Option<u32>,
}

impl StdoutLogConfig {
    pub fn validate(&self) -> crate::Result<()> {
        if self.path.as_os_str().is_empty() {
            public_bail!("mcp stdout_log.path must not be empty");
        }
        if self
            .path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            public_bail!("mcp stdout_log.path must not contain `..` segments");
        }
        if self.max_bytes_per_part == 0 {
            public_bail!("mcp stdout_log.max_bytes_per_part must be >= 1");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub(super) path: Option<PathBuf>,
    pub(super) client: ClientConfig,
    pub(super) servers: BTreeMap<ServerName, ServerConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerConfig {
    Stdio(StdioServerConfig),
    Unix(UnixServerConfig),
    StreamableHttp(StreamableHttpServerConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioServerConfig {
    argv: Vec<String>,
    /// When true, inherit the current process environment when spawning a
    /// `transport=stdio` server.
    ///
    /// Default: `false` for `transport=stdio` (safer-by-default).
    ///
    /// When false, the child environment is cleared and only a small set of
    /// non-secret baseline variables are propagated (plus any `env` entries).
    inherit_env: bool,
    env: BTreeMap<String, String>,
    stdout_log: Option<StdoutLogConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixServerConfig {
    unix_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamableHttpServerConfig {
    urls: StreamableHttpUrls,
    bearer_token_env_var: Option<String>,
    http_headers: BTreeMap<String, String>,
    env_http_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy)]
pub struct StdioServerConfigRef<'a> {
    inner: &'a StdioServerConfig,
}

#[derive(Debug)]
pub struct StdioServerConfigMut<'a> {
    inner: &'a mut StdioServerConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct UnixServerConfigRef<'a> {
    inner: &'a UnixServerConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamableHttpServerConfigRef<'a> {
    inner: &'a StreamableHttpServerConfig,
}

#[derive(Debug)]
pub struct StreamableHttpServerConfigMut<'a> {
    inner: &'a mut StreamableHttpServerConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamableHttpUrls {
    Single { url: String },
    Split { sse_url: String, http_url: String },
}

fn empty_kv_map() -> &'static BTreeMap<String, String> {
    static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeMap::new)
}

fn is_reserved_streamable_http_header(header: &HeaderName) -> bool {
    header
        .as_str()
        .eq_ignore_ascii_case(MCP_PROTOCOL_VERSION_HEADER)
        || header.as_str().eq_ignore_ascii_case(AUTHORIZATION_HEADER)
}

fn is_reserved_streamable_http_env_header(header: &HeaderName) -> bool {
    is_reserved_streamable_http_header(header)
}

fn validate_streamable_http_url_syntax(url_field: &str, url: &str) -> anyhow::Result<()> {
    reqwest::Url::parse(url).map(|_| ()).map_err(|err| {
        wrap_kind(
            ErrorKind::Config,
            anyhow::Error::new(err).context(format!(
                "mcp server transport=streamable_http: invalid {url_field} (url redacted)"
            )),
        )
    })
}

impl ServerConfig {
    pub fn stdio(argv: Vec<String>) -> crate::Result<Self> {
        validate_argv(Transport::Stdio, &argv)?;
        Ok(Self::Stdio(StdioServerConfig {
            argv,
            inherit_env: false,
            env: BTreeMap::new(),
            stdout_log: None,
        }))
    }

    pub fn unix(unix_path: PathBuf) -> crate::Result<Self> {
        if unix_path.as_os_str().is_empty() {
            public_bail!("mcp server transport=unix: unix_path must not be empty");
        }
        Ok(Self::Unix(UnixServerConfig { unix_path }))
    }

    pub fn streamable_http(url: impl Into<String>) -> crate::Result<Self> {
        let url = url.into();
        if url.trim().is_empty() {
            public_bail!("mcp server transport=streamable_http: url must not be empty");
        }
        validate_streamable_http_url_syntax("url", &url)?;
        Ok(Self::StreamableHttp(StreamableHttpServerConfig {
            urls: StreamableHttpUrls::Single { url },
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
        }))
    }

    pub fn streamable_http_split(
        sse_url: impl Into<String>,
        http_url: impl Into<String>,
    ) -> crate::Result<Self> {
        let sse_url = sse_url.into();
        let http_url = http_url.into();
        if sse_url.trim().is_empty() {
            public_bail!("mcp server transport=streamable_http: sse_url must not be empty");
        }
        if http_url.trim().is_empty() {
            public_bail!("mcp server transport=streamable_http: http_url must not be empty");
        }
        validate_streamable_http_url_syntax("sse_url", &sse_url)?;
        validate_streamable_http_url_syntax("http_url", &http_url)?;
        Ok(Self::StreamableHttp(StreamableHttpServerConfig {
            urls: StreamableHttpUrls::Split { sse_url, http_url },
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
        }))
    }

    pub fn transport(&self) -> Transport {
        match self {
            Self::Stdio(_) => Transport::Stdio,
            Self::Unix(_) => Transport::Unix,
            Self::StreamableHttp(_) => Transport::StreamableHttp,
        }
    }

    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::Stdio(cfg) => {
                validate_argv(Transport::Stdio, &cfg.argv)?;
                for key in cfg.env.keys() {
                    if key.trim().is_empty() {
                        public_bail!("mcp server transport=stdio: env key must not be empty");
                    }
                }
                if let Some(log) = cfg.stdout_log.as_ref() {
                    log.validate()?;
                }
            }
            Self::Unix(cfg) => {
                if cfg.unix_path.as_os_str().is_empty() {
                    public_bail!("mcp server transport=unix: unix_path must not be empty");
                }
            }
            Self::StreamableHttp(cfg) => {
                match &cfg.urls {
                    StreamableHttpUrls::Single { url } => {
                        if url.trim().is_empty() {
                            public_bail!(
                                "mcp server transport=streamable_http: url must not be empty"
                            );
                        }
                        validate_streamable_http_url_syntax("url", url)?;
                    }
                    StreamableHttpUrls::Split { sse_url, http_url } => {
                        if sse_url.trim().is_empty() {
                            public_bail!(
                                "mcp server transport=streamable_http: sse_url must not be empty"
                            );
                        }
                        if http_url.trim().is_empty() {
                            public_bail!(
                                "mcp server transport=streamable_http: http_url must not be empty"
                            );
                        }
                        validate_streamable_http_url_syntax("sse_url", sse_url)?;
                        validate_streamable_http_url_syntax("http_url", http_url)?;
                    }
                }

                if let Some(env_var) = cfg.bearer_token_env_var.as_deref() {
                    if env_var.trim().is_empty() {
                        public_bail!(
                            "mcp server transport=streamable_http: bearer_token_env_var must not be empty"
                        );
                    }
                }

                for (header, value) in cfg.http_headers.iter() {
                    if header.trim().is_empty() {
                        public_bail!(
                            "mcp server transport=streamable_http: http_headers key must not be empty"
                        );
                    }
                    let header_name = HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                        wrap_kind(
                            ErrorKind::Config,
                            anyhow::anyhow!(
                                "mcp server transport=streamable_http: invalid http_headers key: {header}"
                            ),
                        )
                    })?;
                    if is_reserved_streamable_http_header(&header_name) {
                        public_bail!(
                            "mcp server transport=streamable_http: http_headers key is reserved by transport: {header}"
                        );
                    }
                    if value.trim().is_empty() {
                        public_bail!(
                            "mcp server transport=streamable_http: http_headers[{header}] must not be empty"
                        );
                    }
                    HeaderValue::from_str(value).map_err(|_| {
                        wrap_kind(
                            ErrorKind::Config,
                            anyhow::anyhow!(
                                "mcp server transport=streamable_http: invalid http_headers[{header}] value"
                            ),
                        )
                    })?;
                }
                for (header, env_var) in cfg.env_http_headers.iter() {
                    if header.trim().is_empty() {
                        public_bail!(
                            "mcp server transport=streamable_http: env_http_headers key must not be empty"
                        );
                    }
                    let header_name = HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                        wrap_kind(
                            ErrorKind::Config,
                            anyhow::anyhow!(
                                "mcp server transport=streamable_http: invalid env_http_headers key: {header}"
                            ),
                        )
                    })?;
                    if is_reserved_streamable_http_env_header(&header_name) {
                        public_bail!(
                            "mcp server transport=streamable_http: env_http_headers key is reserved by transport: {header}"
                        );
                    }
                    if env_var.trim().is_empty() {
                        public_bail!(
                            "mcp server transport=streamable_http: env_http_headers[{header}] must not be empty"
                        );
                    }
                }
            }
        };

        Ok(())
    }

    pub fn as_stdio(&self) -> Option<StdioServerConfigRef<'_>> {
        match self {
            Self::Stdio(cfg) => Some(StdioServerConfigRef { inner: cfg }),
            _ => None,
        }
    }

    pub fn as_stdio_mut(&mut self) -> Option<StdioServerConfigMut<'_>> {
        match self {
            Self::Stdio(cfg) => Some(StdioServerConfigMut { inner: cfg }),
            _ => None,
        }
    }

    pub fn as_unix(&self) -> Option<UnixServerConfigRef<'_>> {
        match self {
            Self::Unix(cfg) => Some(UnixServerConfigRef { inner: cfg }),
            _ => None,
        }
    }

    pub fn as_streamable_http(&self) -> Option<StreamableHttpServerConfigRef<'_>> {
        match self {
            Self::StreamableHttp(cfg) => Some(StreamableHttpServerConfigRef { inner: cfg }),
            _ => None,
        }
    }

    pub fn as_streamable_http_mut(&mut self) -> Option<StreamableHttpServerConfigMut<'_>> {
        match self {
            Self::StreamableHttp(cfg) => Some(StreamableHttpServerConfigMut { inner: cfg }),
            _ => None,
        }
    }

    pub fn argv(&self) -> &[String] {
        self.as_stdio().map_or(&[], StdioServerConfigRef::argv)
    }

    pub fn inherit_env(&self) -> bool {
        self.as_stdio()
            .is_none_or(StdioServerConfigRef::inherit_env)
    }

    pub fn unix_path(&self) -> Option<&Path> {
        self.as_unix().map(UnixServerConfigRef::unix_path)
    }

    pub(crate) fn unix_path_required(&self) -> &Path {
        match self {
            Self::Unix(cfg) => cfg.unix_path.as_path(),
            _ => unreachable!("unix_path_required called for non-unix transport"),
        }
    }

    pub fn url(&self) -> Option<&str> {
        self.as_streamable_http()
            .and_then(StreamableHttpServerConfigRef::url)
    }

    pub fn sse_url(&self) -> Option<&str> {
        self.as_streamable_http()
            .and_then(StreamableHttpServerConfigRef::sse_url)
    }

    pub fn http_url(&self) -> Option<&str> {
        self.as_streamable_http()
            .and_then(StreamableHttpServerConfigRef::http_url)
    }

    pub fn bearer_token_env_var(&self) -> Option<&str> {
        self.as_streamable_http()
            .and_then(StreamableHttpServerConfigRef::bearer_token_env_var)
    }

    pub fn http_headers(&self) -> &BTreeMap<String, String> {
        match self.as_streamable_http() {
            Some(cfg) => cfg.http_headers(),
            None => empty_kv_map(),
        }
    }

    pub fn env_http_headers(&self) -> &BTreeMap<String, String> {
        match self.as_streamable_http() {
            Some(cfg) => cfg.env_http_headers(),
            None => empty_kv_map(),
        }
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        match self.as_stdio() {
            Some(cfg) => cfg.env(),
            None => empty_kv_map(),
        }
    }

    pub fn stdout_log(&self) -> Option<&StdoutLogConfig> {
        self.as_stdio().and_then(StdioServerConfigRef::stdout_log)
    }

    pub fn set_inherit_env(&mut self, inherit_env: bool) -> crate::Result<()> {
        if let Some(mut cfg) = self.as_stdio_mut() {
            return cfg.set_inherit_env(inherit_env);
        }
        match self.transport() {
            Transport::Unix => {
                if !inherit_env {
                    public_bail!("mcp server transport=unix: inherit_env must be true");
                }
            }
            Transport::StreamableHttp => {
                if !inherit_env {
                    public_bail!("mcp server transport=streamable_http: inherit_env must be true");
                }
            }
            Transport::Stdio => unreachable!("stdio transport must yield as_stdio_mut"),
        }
        Ok(())
    }

    pub fn set_bearer_token_env_var(
        &mut self,
        bearer_token_env_var: Option<String>,
    ) -> crate::Result<()> {
        match self.as_streamable_http_mut() {
            Some(mut cfg) => cfg.set_bearer_token_env_var(bearer_token_env_var),
            _ => {
                if bearer_token_env_var.is_some() {
                    public_bail!(
                        "mcp server transport={}: bearer_token_env_var is not allowed",
                        transport_tag(self.transport())
                    );
                }
                Ok(())
            }
        }
    }

    pub fn env_mut(&mut self) -> crate::Result<&mut BTreeMap<String, String>> {
        let transport = self.transport();
        match self {
            Self::Stdio(cfg) => Ok(&mut cfg.env),
            _ => public_bail!(
                "mcp server transport={}: env is not allowed",
                transport_tag(transport)
            ),
        }
    }

    pub fn http_headers_mut(&mut self) -> crate::Result<&mut BTreeMap<String, String>> {
        let transport = self.transport();
        match self {
            Self::StreamableHttp(cfg) => Ok(&mut cfg.http_headers),
            _ => public_bail!(
                "mcp server transport={}: http_headers are not allowed",
                transport_tag(transport)
            ),
        }
    }

    pub fn env_http_headers_mut(&mut self) -> crate::Result<&mut BTreeMap<String, String>> {
        let transport = self.transport();
        match self {
            Self::StreamableHttp(cfg) => Ok(&mut cfg.env_http_headers),
            _ => public_bail!(
                "mcp server transport={}: env_http_headers are not allowed",
                transport_tag(transport)
            ),
        }
    }

    pub fn set_stdout_log(&mut self, stdout_log: Option<StdoutLogConfig>) -> crate::Result<()> {
        match self.as_stdio_mut() {
            Some(mut cfg) => cfg.set_stdout_log(stdout_log),
            _ => {
                if stdout_log.is_some() {
                    public_bail!(
                        "mcp server transport={}: stdout_log is not allowed",
                        transport_tag(self.transport())
                    );
                }
                Ok(())
            }
        }
    }
}

impl<'a> StdioServerConfigRef<'a> {
    pub fn argv(self) -> &'a [String] {
        &self.inner.argv
    }

    pub fn inherit_env(self) -> bool {
        self.inner.inherit_env
    }

    pub fn env(self) -> &'a BTreeMap<String, String> {
        &self.inner.env
    }

    pub fn stdout_log(self) -> Option<&'a StdoutLogConfig> {
        self.inner.stdout_log.as_ref()
    }
}

impl<'a> StdioServerConfigMut<'a> {
    pub fn argv(&self) -> &[String] {
        &self.inner.argv
    }

    pub fn inherit_env(&self) -> bool {
        self.inner.inherit_env
    }

    pub fn set_inherit_env(&mut self, inherit_env: bool) -> crate::Result<()> {
        self.inner.inherit_env = inherit_env;
        Ok(())
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.inner.env
    }

    pub fn env_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.inner.env
    }

    pub fn stdout_log(&self) -> Option<&StdoutLogConfig> {
        self.inner.stdout_log.as_ref()
    }

    pub fn set_stdout_log(&mut self, stdout_log: Option<StdoutLogConfig>) -> crate::Result<()> {
        self.inner.stdout_log = stdout_log;
        Ok(())
    }
}

impl<'a> UnixServerConfigRef<'a> {
    pub fn unix_path(self) -> &'a Path {
        self.inner.unix_path.as_path()
    }
}

impl<'a> StreamableHttpServerConfigRef<'a> {
    pub fn url(self) -> Option<&'a str> {
        match &self.inner.urls {
            StreamableHttpUrls::Single { url } => Some(url.as_str()),
            StreamableHttpUrls::Split { .. } => None,
        }
    }

    pub fn sse_url(self) -> Option<&'a str> {
        match &self.inner.urls {
            StreamableHttpUrls::Single { .. } => None,
            StreamableHttpUrls::Split { sse_url, .. } => Some(sse_url.as_str()),
        }
    }

    pub fn http_url(self) -> Option<&'a str> {
        match &self.inner.urls {
            StreamableHttpUrls::Single { .. } => None,
            StreamableHttpUrls::Split { http_url, .. } => Some(http_url.as_str()),
        }
    }

    pub fn bearer_token_env_var(self) -> Option<&'a str> {
        self.inner.bearer_token_env_var.as_deref()
    }

    pub fn http_headers(self) -> &'a BTreeMap<String, String> {
        &self.inner.http_headers
    }

    pub fn env_http_headers(self) -> &'a BTreeMap<String, String> {
        &self.inner.env_http_headers
    }
}

impl<'a> StreamableHttpServerConfigMut<'a> {
    pub fn url(&self) -> Option<&str> {
        StreamableHttpServerConfigRef { inner: self.inner }.url()
    }

    pub fn sse_url(&self) -> Option<&str> {
        StreamableHttpServerConfigRef { inner: self.inner }.sse_url()
    }

    pub fn http_url(&self) -> Option<&str> {
        StreamableHttpServerConfigRef { inner: self.inner }.http_url()
    }

    pub fn bearer_token_env_var(&self) -> Option<&str> {
        self.inner.bearer_token_env_var.as_deref()
    }

    pub fn set_bearer_token_env_var(
        &mut self,
        bearer_token_env_var: Option<String>,
    ) -> crate::Result<()> {
        self.inner.bearer_token_env_var = bearer_token_env_var;
        Ok(())
    }

    pub fn http_headers(&self) -> &BTreeMap<String, String> {
        &self.inner.http_headers
    }

    pub fn http_headers_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.inner.http_headers
    }

    pub fn env_http_headers(&self) -> &BTreeMap<String, String> {
        &self.inner.env_http_headers
    }

    pub fn env_http_headers_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.inner.env_http_headers
    }
}

fn validate_argv(transport: Transport, argv: &[String]) -> anyhow::Result<()> {
    if argv.is_empty() {
        return Err(tagged_message(
            ErrorKind::Config,
            format!(
                "mcp server transport={}: argv must not be empty",
                transport_tag(transport)
            ),
        ));
    }
    for (idx, arg) in argv.iter().enumerate() {
        if arg.trim().is_empty() {
            return Err(tagged_message(
                ErrorKind::Config,
                format!(
                    "mcp server transport={}: argv[{idx}] must not be empty",
                    transport_tag(transport)
                ),
            ));
        }
    }
    Ok(())
}

fn transport_tag(transport: Transport) -> &'static str {
    match transport {
        Transport::Stdio => "stdio",
        Transport::Unix => "unix",
        Transport::StreamableHttp => "streamable_http",
    }
}

fn absolutize_config_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

impl Config {
    pub fn new(client: ClientConfig, servers: BTreeMap<ServerName, ServerConfig>) -> Self {
        Self {
            path: None,
            client,
            servers,
        }
    }

    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(absolutize_config_path(path));
        self
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn thread_root(&self) -> Option<&Path> {
        self.path().and_then(Path::parent)
    }

    pub fn client(&self) -> &ClientConfig {
        &self.client
    }

    pub fn servers(&self) -> &BTreeMap<ServerName, ServerConfig> {
        &self.servers
    }

    pub fn validate(&self) -> crate::Result<()> {
        self.client.validate().map_err(|err| {
            let msg = format!("invalid mcp client config: {err}");
            err.context(msg)
        })?;
        for (name, server) in self.servers.iter() {
            server.validate().map_err(|err| {
                let msg = format!("invalid mcp server config (server={name}): {err}");
                err.context(msg)
            })?;
        }
        Ok(())
    }

    pub fn server(&self, name: &str) -> Option<&ServerConfig> {
        let name = ServerName::parse(name).ok()?;
        self.servers.get(&name)
    }

    pub fn server_named(&self, name: &ServerName) -> Option<&ServerConfig> {
        self.servers.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;

    #[test]
    fn streamable_http_constructor_rejects_invalid_url_syntax() {
        let err =
            ServerConfig::streamable_http("https://exa mple.invalid").expect_err("invalid url");
        let message = err.to_string();
        assert!(message.contains("invalid url"), "{message}");
        assert!(
            !message.contains("exa mple.invalid"),
            "url should stay redacted"
        );
    }

    #[test]
    fn streamable_http_split_constructor_rejects_invalid_url_syntax() {
        let err = ServerConfig::streamable_http_split("https://example.invalid/sse", "not a url")
            .expect_err("invalid split url");
        let message = err.to_string();
        assert!(message.contains("invalid http_url"), "{message}");
        assert!(!message.contains("not a url"), "url should stay redacted");
    }
}
