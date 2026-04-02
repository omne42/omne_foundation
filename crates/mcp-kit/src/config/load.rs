use std::collections::{BTreeMap, btree_map::Entry};
use std::path::{Path, PathBuf};

use anyhow::Context;
use config_kit::{ConfigFormat, ConfigLoadOptions, SchemaConfigLoader, SchemaFileLayerOptions};
use serde_json::Value;

use super::file_format::{ConfigFile, StdoutLogConfigFile};
use super::{ClientConfig, Config, ServerConfig, StdoutLogConfig, Transport};
use crate::ServerName;

const MCP_CONFIG_VERSION: u32 = 1;
const DEFAULT_CONFIG_CANDIDATES: [&str; 2] = [".mcp.json", "mcp.json"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConfigLoadPolicy {
    allow_override_outside_root: bool,
}

impl ConfigLoadPolicy {
    #[must_use]
    pub fn allow_override_outside_root(mut self, allow: bool) -> Self {
        self.allow_override_outside_root = allow;
        self
    }

    #[must_use]
    pub fn allows_override_outside_root(self) -> bool {
        self.allow_override_outside_root
    }
}

fn config_load_options() -> ConfigLoadOptions {
    ConfigLoadOptions::new()
        .with_format(ConfigFormat::Json)
        .with_max_bytes(super::MAX_CONFIG_BYTES)
}

fn schema_file_layer_options(required: bool) -> SchemaFileLayerOptions {
    SchemaFileLayerOptions::new()
        .required(required)
        .with_load_options(config_load_options())
}

fn load_json_value_from_file(path: &Path) -> anyhow::Result<Value> {
    Ok(SchemaConfigLoader::new()
        .add_file_layer("mcp config", path, schema_file_layer_options(true))
        .load::<Value>()?
        .into_value())
}

fn ensure_absolute_thread_root(thread_root: &Path) -> anyhow::Result<()> {
    if thread_root.is_absolute() {
        return Ok(());
    }
    anyhow::bail!(
        "mcp config root must be absolute to avoid cwd-dependent path drift: {}",
        thread_root.display()
    );
}

fn ensure_candidate_discovery_root_exists(thread_root: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(thread_root) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(err).with_context(|| {
            format!(
                "inspect MCP config root before candidate discovery: {}",
                thread_root.display()
            )
        }),
        Err(err) => Err(err).with_context(|| {
            format!(
                "inspect MCP config root before candidate discovery: {}",
                thread_root.display()
            )
        }),
    }
}

fn canonicalize_existing_ancestor(path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut cursor = Some(path);
    while let Some(candidate) = cursor {
        match std::fs::canonicalize(candidate) {
            Ok(canonical) => return Ok(Some(canonical)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                cursor = candidate.parent();
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "canonicalize existing ancestor for MCP config boundary check: {}",
                        path.display()
                    )
                });
            }
        }
    }
    Ok(None)
}

fn resolve_override_path(
    thread_root: &Path,
    override_path: PathBuf,
    policy: ConfigLoadPolicy,
) -> anyhow::Result<PathBuf> {
    let path = if override_path.is_absolute() {
        override_path
    } else {
        thread_root.join(override_path)
    };

    if policy.allows_override_outside_root() {
        return Ok(path);
    }

    let canonical_root = match std::fs::canonicalize(thread_root) {
        Ok(canonical_root) => canonical_root,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => thread_root.to_path_buf(),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "canonicalize MCP config root for override boundary check: {}",
                    thread_root.display()
                )
            });
        }
    };
    if let Some(canonical_override_or_parent) = canonicalize_existing_ancestor(&path)?
        && !canonical_override_or_parent.starts_with(&canonical_root)
    {
        anyhow::bail!(
            "override config path must be within root {} (set ConfigLoadPolicy::allow_override_outside_root(true) to override): {}",
            thread_root.display(),
            path.display()
        );
    }

    Ok(path)
}

async fn load_initial_path_and_value(
    thread_root: &Path,
    override_path: Option<PathBuf>,
    policy: ConfigLoadPolicy,
) -> anyhow::Result<Option<(PathBuf, Value)>> {
    match override_path {
        Some(path) => {
            let path = resolve_override_path(thread_root, path, policy)?;
            let value = load_json_value_from_file(&path)?;
            Ok(Some((path, value)))
        }
        None => {
            ensure_candidate_discovery_root_exists(thread_root)?;
            Ok(SchemaConfigLoader::new()
                .add_candidate_file_layer(
                    "mcp config",
                    thread_root,
                    DEFAULT_CONFIG_CANDIDATES,
                    schema_file_layer_options(false),
                )
                .load_optional::<Value>()?
                .map(|loaded| {
                    let path = loaded
                        .layers()
                        .last()
                        .and_then(|layer| layer.path())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "mcp config loader returned a candidate file layer without a path"
                            )
                        })?
                        .to_path_buf();
                    Ok::<_, anyhow::Error>((path, loaded.into_value()))
                })
                .transpose()?)
        }
    }
}

fn parse_config_file(path: Option<&Path>, json: Value) -> anyhow::Result<ConfigFile> {
    fn parse_context(path: Option<&Path>) -> String {
        match path {
            Some(path) => format!("parse {}", path.display()),
            None => "parse mcp config".to_string(),
        }
    }

    match json {
        Value::Object(root) => {
            if root.contains_key("mcpServers") {
                anyhow::bail!(
                    "unsupported legacy MCP config format: `mcpServers` wrapper is no longer accepted; use canonical mcp.json v{MCP_CONFIG_VERSION}"
                );
            }
            if !root.contains_key("version") {
                anyhow::bail!(
                    "unsupported mcp.json format: missing `version` (expected v{MCP_CONFIG_VERSION})"
                );
            }

            serde_json::from_value(Value::Object(root)).with_context(|| parse_context(path))
        }
        _ => anyhow::bail!("invalid mcp config: expected a JSON object"),
    }
}

fn resolve_inherit_env(
    name: &str,
    transport: Transport,
    inherit_env: Option<bool>,
) -> anyhow::Result<bool> {
    match transport {
        Transport::Stdio => Ok(inherit_env.unwrap_or(false)),
        _ => {
            if inherit_env.is_some() {
                anyhow::bail!("mcp server {name}: inherit_env is only valid for transport=stdio");
            }
            Ok(true)
        }
    }
}

fn ensure_unix_path_only_for_unix(name: &str, unix_path_present: bool) -> anyhow::Result<()> {
    if unix_path_present {
        anyhow::bail!("mcp server {name}: unix_path is only valid for transport=unix");
    }
    Ok(())
}

fn ensure_url_fields_only_for_streamable_http(
    name: &str,
    has_url_fields: bool,
) -> anyhow::Result<()> {
    if has_url_fields {
        anyhow::bail!(
            "mcp server {name}: url/sse_url/http_url are only valid for transport=streamable_http"
        );
    }
    Ok(())
}

fn ensure_http_headers_auth_only_for_streamable_http(
    name: &str,
    has_auth_fields: bool,
) -> anyhow::Result<()> {
    if has_auth_fields {
        anyhow::bail!(
            "mcp server {name}: http headers/auth are only valid for transport=streamable_http"
        );
    }
    Ok(())
}

fn env_secret_spec(env_var: String) -> String {
    format!("secret://env/{env_var}")
}

fn coalesce_bearer_token_secret(
    name: &str,
    bearer_token_secret: Option<String>,
    bearer_token_env_var: Option<String>,
) -> anyhow::Result<Option<String>> {
    match (bearer_token_secret, bearer_token_env_var) {
        (Some(_), Some(_)) => anyhow::bail!(
            "mcp server {name}: set either bearer_token_secret or legacy bearer_token_env_var, not both"
        ),
        (Some(secret), None) => Ok(Some(secret)),
        (None, Some(env_var)) => Ok(Some(env_secret_spec(env_var))),
        (None, None) => Ok(None),
    }
}

fn coalesce_secret_http_headers(
    name: &str,
    secret_http_headers: BTreeMap<String, String>,
    env_http_headers: BTreeMap<String, String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut merged = secret_http_headers;
    for (header, env_var) in env_http_headers {
        match merged.entry(header) {
            Entry::Occupied(entry) => {
                anyhow::bail!(
                    "mcp server {name}: set either secret_http_headers[{0}] or legacy env_http_headers[{0}], not both",
                    entry.key()
                );
            }
            Entry::Vacant(entry) => {
                entry.insert(env_secret_spec(env_var));
            }
        }
    }
    Ok(merged)
}

fn ensure_env_empty(name: &str, transport: Transport, env_nonempty: bool) -> anyhow::Result<()> {
    if !env_nonempty {
        return Ok(());
    }
    match transport {
        Transport::Unix => {
            anyhow::bail!("mcp server {name}: env is not supported for transport=unix")
        }
        Transport::StreamableHttp => {
            anyhow::bail!("mcp server {name}: env is not supported for transport=streamable_http")
        }
        Transport::Stdio => Ok(()),
    }
}

fn ensure_stdout_log_supported(
    name: &str,
    transport: Transport,
    stdout_log_present: bool,
) -> anyhow::Result<()> {
    if !stdout_log_present {
        return Ok(());
    }
    match transport {
        Transport::Unix => {
            anyhow::bail!("mcp server {name}: stdout_log is not supported for transport=unix")
        }
        Transport::StreamableHttp => anyhow::bail!(
            "mcp server {name}: stdout_log is not supported for transport=streamable_http"
        ),
        Transport::Stdio => Ok(()),
    }
}

fn insert_server_unique(
    servers: &mut BTreeMap<ServerName, ServerConfig>,
    raw_name: &str,
    server_name: ServerName,
    server_cfg: ServerConfig,
) -> anyhow::Result<()> {
    match servers.entry(server_name) {
        Entry::Vacant(entry) => {
            entry.insert(server_cfg);
            Ok(())
        }
        Entry::Occupied(entry) => {
            anyhow::bail!(
                "duplicate mcp server name after normalization: {raw_name:?} -> {}",
                entry.key()
            );
        }
    }
}

fn build_v1_config(
    thread_root: &Path,
    path: Option<PathBuf>,
    cfg: ConfigFile,
) -> anyhow::Result<Config> {
    if cfg.version != MCP_CONFIG_VERSION {
        anyhow::bail!(
            "unsupported mcp.json version {} (expected {})",
            cfg.version,
            MCP_CONFIG_VERSION
        );
    }

    let client = match cfg.client {
        Some(client) => ClientConfig {
            protocol_version: client.protocol_version,
            capabilities: client.capabilities,
            roots: client.roots,
        },
        None => ClientConfig::default(),
    };
    client.validate().map_err(|err| {
        let msg = format!("invalid mcp.json client config: {err}");
        err.context(msg)
    })?;

    let mut servers = BTreeMap::<ServerName, ServerConfig>::new();
    for (name, server) in cfg.servers {
        let server_name_key = ServerName::parse(&name)
            .with_context(|| format!("invalid mcp server name {name:?}"))?;

        if matches!(
            server.transport,
            Transport::Unix | Transport::StreamableHttp
        ) && server.argv.is_some()
        {
            match server.transport {
                Transport::Unix => {
                    anyhow::bail!("mcp server {name}: argv is not allowed for transport=unix")
                }
                Transport::StreamableHttp => anyhow::bail!(
                    "mcp server {name}: argv is not allowed for transport=streamable_http"
                ),
                Transport::Stdio => unreachable!("matches! guard excludes stdio"),
            }
        }

        let stdout_log = match server.transport {
            Transport::Stdio => server
                .stdout_log
                .map(|log| parse_stdout_log_config(thread_root, &name, log))
                .transpose()?,
            Transport::Unix => {
                ensure_stdout_log_supported(&name, Transport::Unix, server.stdout_log.is_some())?;
                None
            }
            Transport::StreamableHttp => {
                ensure_stdout_log_supported(
                    &name,
                    Transport::StreamableHttp,
                    server.stdout_log.is_some(),
                )?;
                None
            }
        };

        let inherit_env = resolve_inherit_env(&name, server.transport, server.inherit_env)?;

        let argv = match server.transport {
            Transport::Stdio => server.argv.unwrap_or_default(),
            _ => Vec::new(),
        };
        let unix_path = match server.transport {
            Transport::Unix => server.unix_path.map(|unix_path| {
                if unix_path.is_absolute() {
                    unix_path
                } else {
                    thread_root.join(unix_path)
                }
            }),
            _ => server.unix_path,
        };

        let server_cfg = match server.transport {
            Transport::Stdio => {
                ensure_unix_path_only_for_unix(&name, unix_path.is_some())?;
                let has_url_fields =
                    server.url.is_some() || server.sse_url.is_some() || server.http_url.is_some();
                ensure_url_fields_only_for_streamable_http(&name, has_url_fields)?;
                let has_auth_fields = server.bearer_token_secret.is_some()
                    || server.bearer_token_env_var.is_some()
                    || !server.http_headers.is_empty()
                    || !server.secret_http_headers.is_empty()
                    || !server.env_http_headers.is_empty();
                ensure_http_headers_auth_only_for_streamable_http(&name, has_auth_fields)?;

                let mut server_cfg = ServerConfig::stdio(argv)?;
                server_cfg.set_inherit_env(inherit_env)?;
                *server_cfg.env_mut()? = server.env;
                server_cfg.set_stdout_log(stdout_log)?;
                server_cfg
            }
            Transport::Unix => {
                let has_url_fields =
                    server.url.is_some() || server.sse_url.is_some() || server.http_url.is_some();
                ensure_url_fields_only_for_streamable_http(&name, has_url_fields)?;
                let env_nonempty = !server.env.is_empty();
                ensure_env_empty(&name, Transport::Unix, env_nonempty)?;
                let has_auth_fields = server.bearer_token_secret.is_some()
                    || server.bearer_token_env_var.is_some()
                    || !server.http_headers.is_empty()
                    || !server.secret_http_headers.is_empty()
                    || !server.env_http_headers.is_empty();
                ensure_http_headers_auth_only_for_streamable_http(&name, has_auth_fields)?;

                let unix_path = unix_path.ok_or_else(|| {
                    anyhow::anyhow!("mcp server {name}: unix_path is required for transport=unix")
                })?;
                ServerConfig::unix(unix_path)?
            }
            Transport::StreamableHttp => {
                ensure_unix_path_only_for_unix(&name, unix_path.is_some())?;
                let env_nonempty = !server.env.is_empty();
                ensure_env_empty(&name, Transport::StreamableHttp, env_nonempty)?;

                let mut server_cfg = match (server.url, server.sse_url, server.http_url) {
                    (Some(url), None, None) => ServerConfig::streamable_http(url)?,
                    (None, Some(sse_url), Some(http_url)) => {
                        ServerConfig::streamable_http_split(sse_url, http_url)?
                    }
                    (None, None, None) => anyhow::bail!(
                        "mcp server {name}: url (or sse_url + http_url) is required for transport=streamable_http"
                    ),
                    (Some(_), Some(_), _) | (Some(_), _, Some(_)) => anyhow::bail!(
                        "mcp server {name}: set either url or (sse_url + http_url), not both"
                    ),
                    (None, Some(_), None) | (None, None, Some(_)) => {
                        anyhow::bail!("mcp server {name}: sse_url and http_url must both be set")
                    }
                };
                server_cfg.set_bearer_token_secret(coalesce_bearer_token_secret(
                    &name,
                    server.bearer_token_secret,
                    server.bearer_token_env_var,
                )?)?;
                *server_cfg.http_headers_mut()? = server.http_headers;
                *server_cfg.secret_http_headers_mut()? = coalesce_secret_http_headers(
                    &name,
                    server.secret_http_headers,
                    server.env_http_headers,
                )?;
                server_cfg
            }
        };
        server_cfg.validate().map_err(|err| {
            let msg = format!("invalid mcp server config (server={server_name_key}): {err}");
            err.context(msg)
        })?;

        insert_server_unique(&mut servers, &name, server_name_key, server_cfg)?;
    }

    Ok(Config {
        path,
        client,
        servers,
    })
}

fn parse_stdout_log_config(
    thread_root: &Path,
    name: &str,
    log: StdoutLogConfigFile,
) -> anyhow::Result<StdoutLogConfig> {
    let max_bytes_per_part = log
        .max_bytes_per_part
        .unwrap_or(super::DEFAULT_STDOUT_LOG_MAX_BYTES_PER_PART);
    let max_parts = log.max_parts.unwrap_or(super::DEFAULT_STDOUT_LOG_MAX_PARTS);
    let max_parts = if max_parts == 0 {
        None
    } else {
        Some(max_parts.max(1))
    };

    let mut cfg = StdoutLogConfig {
        path: log.path,
        max_bytes_per_part,
        max_parts,
    };
    cfg.validate().map_err(|err| {
        let msg = format!("mcp server {name}: invalid stdout_log config: {err}");
        err.context(msg)
    })?;

    if !cfg.path.is_absolute() {
        cfg.path = thread_root.join(&cfg.path);
    }

    Ok(cfg)
}

impl Config {
    /// Load `mcp.json` (v1), but fail if no config file is found.
    ///
    /// Unlike `Config::load`, this does not treat missing config as "empty config".
    pub async fn load_required(
        thread_root: &Path,
        override_path: Option<PathBuf>,
    ) -> crate::Result<Self> {
        Self::load_required_with_policy(thread_root, override_path, ConfigLoadPolicy::default())
            .await
    }

    /// Load `mcp.json` (v1) with an explicit override-path policy, but fail if no config file is found.
    pub async fn load_required_with_policy(
        thread_root: &Path,
        override_path: Option<PathBuf>,
        policy: ConfigLoadPolicy,
    ) -> crate::Result<Self> {
        let cfg = Self::load_with_policy(thread_root, override_path, policy).await?;
        if cfg.path().is_none() {
            return Err(crate::error::tagged_message(
                crate::error::ErrorKind::Config,
                format!(
                    "mcp config not found under root {} (tried: {})",
                    thread_root.display(),
                    DEFAULT_CONFIG_CANDIDATES.join(", ")
                ),
            )
            .into());
        }
        Ok(cfg)
    }

    pub async fn load(thread_root: &Path, override_path: Option<PathBuf>) -> crate::Result<Self> {
        Self::load_with_policy(thread_root, override_path, ConfigLoadPolicy::default()).await
    }

    /// Load `mcp.json` (v1) with an explicit override-path policy.
    ///
    /// By default `Config::load` and `Config::load_required` reject override paths that escape the
    /// provided root. Callers that intentionally need an external config file must opt into that
    /// behavior explicitly here.
    pub async fn load_with_policy(
        thread_root: &Path,
        override_path: Option<PathBuf>,
        policy: ConfigLoadPolicy,
    ) -> crate::Result<Self> {
        ensure_absolute_thread_root(thread_root).map_err(|err| {
            crate::Error::from(crate::error::tag_anyhow(
                crate::error::ErrorKind::Config,
                err,
            ))
        })?;
        let Some((path, json)) = load_initial_path_and_value(thread_root, override_path, policy)
            .await
            .map_err(|err| {
                crate::Error::from(crate::error::tag_anyhow(
                    crate::error::ErrorKind::Config,
                    err,
                ))
            })?
        else {
            return Ok(Self {
                path: None,
                client: ClientConfig::default(),
                servers: BTreeMap::new(),
            });
        };
        let cfg = parse_config_file(Some(path.as_path()), json).map_err(|err| {
            crate::Error::from(crate::error::tag_anyhow(
                crate::error::ErrorKind::Config,
                err,
            ))
        })?;
        build_v1_config(thread_root, Some(path), cfg).map_err(|err| {
            crate::Error::from(crate::error::tag_anyhow(
                crate::error::ErrorKind::Config,
                err,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    use std::path::PathBuf;

    #[cfg(not(windows))]
    use super::{ConfigLoadPolicy, canonicalize_existing_ancestor, resolve_override_path};

    #[cfg(not(windows))]
    #[test]
    fn canonicalize_existing_ancestor_reports_non_not_found_errors() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("blocked");
        std::fs::write(&path, b"not a directory").unwrap();

        let err = canonicalize_existing_ancestor(&path.join("mcp.json"))
            .expect_err("non-directory ancestor should not be treated as missing");
        assert!(err.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotADirectory)
        }));
    }

    #[cfg(not(windows))]
    #[test]
    fn resolve_override_path_reports_non_not_found_boundary_errors() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("blocked"), b"not a directory").unwrap();

        let err = resolve_override_path(
            &root,
            PathBuf::from("blocked/mcp.json"),
            ConfigLoadPolicy::default(),
        )
        .expect_err("non-directory override prefix should not be treated as missing");
        assert!(err.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotADirectory)
        }));
    }
}
