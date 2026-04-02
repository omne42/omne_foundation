#[cfg(feature = "cli")]
mod cli_tests {
    use std::fs;

    use assert_cmd::cargo::cargo_bin_cmd;
    use predicates::prelude::*;
    use serde_json::Value;

    #[test]
    fn trust_requires_yes_trust() {
        let dir = tempfile::tempdir().unwrap();

        let mut cmd = cargo_bin_cmd!("mcpctl");
        cmd.arg("--root")
            .arg(dir.path())
            .arg("--trust")
            .arg("list-servers");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("--yes-trust"));
    }

    #[test]
    fn allow_host_with_no_dns_check_warns() {
        let dir = tempfile::tempdir().unwrap();

        let mut cmd = cargo_bin_cmd!("mcpctl");
        cmd.arg("--root")
            .arg(dir.path())
            .arg("--allow-host")
            .arg("example.com")
            .arg("--no-dns-check")
            .arg("list-servers");
        cmd.assert()
            .success()
            .stderr(predicate::str::contains(
                "WARNING: --allow-host is set with DNS checks disabled (--no-dns-check).",
            ))
            .stderr(
                predicate::str::contains(
                    "NOTE: enabling DNS checks because --allow-host was provided.",
                )
                .not(),
            );
    }

    #[test]
    fn config_outside_root_missing_path_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let missing = outside.path().join("missing-config.json");

        let mut cmd = cargo_bin_cmd!("mcpctl");
        cmd.arg("--root")
            .arg(root.path())
            .arg("--config")
            .arg(&missing)
            .arg("list-servers");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("--config must be within --root"));
    }

    #[test]
    fn list_servers_omits_pseudo_defaults_for_non_matching_transports() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("mcp.json"),
            r#"
{
  "version": 1,
  "servers": {
    "stdio": { "transport": "stdio", "argv": ["stdio-server"], "env": { "NO_COLOR": "1" } },
    "unix": { "transport": "unix", "unix_path": "/tmp/mcp.sock" },
    "http": { "transport": "streamable_http", "url": "https://example.com/mcp" }
  }
}
"#,
        )
        .unwrap();

        let mut cmd = cargo_bin_cmd!("mcpctl");
        let output = cmd
            .arg("--root")
            .arg(root.path())
            .arg("list-servers")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let body: Value = serde_json::from_slice(&output).unwrap();
        let servers = body["servers"].as_array().unwrap();
        let unix = servers
            .iter()
            .find(|server| server["name"] == "unix")
            .expect("unix server");
        assert!(unix["argv_program"].is_null());
        assert!(unix["inherit_env"].is_null());
        assert!(unix["env_keys"].is_null());

        let http = servers
            .iter()
            .find(|server| server["name"] == "http")
            .expect("http server");
        assert!(http["argv_program"].is_null());
        assert!(http["inherit_env"].is_null());
        assert!(http["env_keys"].is_null());
        assert!(http["http_header_keys"].is_array());
    }
}
