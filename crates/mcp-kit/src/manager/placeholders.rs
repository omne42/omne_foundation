use std::ffi::OsString;
use std::path::Path;

use anyhow::Context;
use tokio::process::Command;

const STDIO_BASELINE_ENV_VARS: [&str; 8] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SystemRoot",
    "SYSTEMROOT",
];
pub(super) fn apply_stdio_baseline_env(cmd: &mut Command) {
    for key in STDIO_BASELINE_ENV_VARS {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
}

fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

enum PlaceholderChunk<'a> {
    Text(&'a str),
    Placeholder(&'a str),
}

fn parse_placeholder_segments(template: &str) -> anyhow::Result<Vec<PlaceholderChunk<'_>>> {
    let mut chunks = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        chunks.push(PlaceholderChunk::Text(&rest[..start]));
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| anyhow::anyhow!("unterminated placeholder (missing `}}`)"))?;
        let name = &after[..end];
        if !is_env_var_name(name) {
            anyhow::bail!("invalid placeholder name: {name}");
        }
        chunks.push(PlaceholderChunk::Placeholder(name));
        rest = &after[end + 1..];
    }
    chunks.push(PlaceholderChunk::Text(rest));
    Ok(chunks)
}

fn expand_placeholders_trusted_with_mode(
    template: &str,
    cwd: &Path,
    allow_env_vars: bool,
) -> anyhow::Result<String> {
    if !template.contains("${") {
        return Ok(template.to_string());
    }

    let mut out = String::with_capacity(template.len());
    for chunk in parse_placeholder_segments(template)? {
        match chunk {
            PlaceholderChunk::Text(segment) => out.push_str(segment),
            PlaceholderChunk::Placeholder(name) => {
                let value = match name {
                    "CLAUDE_PLUGIN_ROOT" | "MCP_ROOT" => {
                        cwd.to_str().map(str::to_owned).ok_or_else(|| {
                            anyhow::anyhow!("placeholder `{name}` requires a UTF-8 cwd")
                        })?
                    }
                    _ if allow_env_vars => {
                        std::env::var(name).with_context(|| format!("read env var: {name}"))?
                    }
                    _ => anyhow::bail!(
                        "placeholder `{name}` is not allowed in this transport field; only ${{MCP_ROOT}} and ${{CLAUDE_PLUGIN_ROOT}} are supported"
                    ),
                };
                out.push_str(&value);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(super) fn expand_placeholders_trusted(template: &str, cwd: &Path) -> anyhow::Result<String> {
    expand_placeholders_trusted_with_mode(template, cwd, true)
}

pub(super) fn expand_root_placeholders_trusted(
    template: &str,
    cwd: &Path,
) -> anyhow::Result<String> {
    expand_placeholders_trusted_with_mode(template, cwd, false)
}

pub(super) fn expand_placeholders_trusted_os(
    template: &str,
    cwd: &Path,
) -> anyhow::Result<OsString> {
    if !template.contains("${") {
        return Ok(OsString::from(template));
    }

    let mut out = OsString::new();
    for chunk in parse_placeholder_segments(template)? {
        match chunk {
            PlaceholderChunk::Text(segment) => out.push(segment),
            PlaceholderChunk::Placeholder(name) => {
                let value = match name {
                    "CLAUDE_PLUGIN_ROOT" | "MCP_ROOT" => cwd.as_os_str().to_os_string(),
                    _ => std::env::var_os(name)
                        .ok_or_else(|| anyhow::anyhow!("read env var: {name}"))?,
                };
                out.push(value);
            }
        }
    }
    Ok(out)
}
