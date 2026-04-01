#![allow(dead_code)]

use std::{
    collections::BTreeSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[cfg(test)]
use policy_meta::POLICY_META_SCHEMA_FILE;
use policy_meta::{
    POLICY_META_TYPES_FILE, PolicyProfileV1, policy_meta_typescript_bindings, profile_documents,
    schema_documents,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactKind {
    Schema,
    TypescriptBinding,
    Profile,
}

impl ArtifactKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::TypescriptBinding => "typescript binding",
            Self::Profile => "profile",
        }
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ArtifactError {
    #[error("failed to create artifact dir {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("failed to read artifact dir {path}: {source}")]
    ReadDir { path: PathBuf, source: io::Error },
    #[error("failed to inspect artifact dir {path}: {source}")]
    InspectDir { path: PathBuf, source: io::Error },
    #[error("failed to read {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("failed to remove stale generated artifact {path}: {source}")]
    RemoveArtifact { path: PathBuf, source: io::Error },
    #[error("failed to parse {path}: {source}")]
    ParseJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to render schema: {source}")]
    RenderSchema { source: serde_json::Error },
    #[error("failed to render profile: {source}")]
    RenderProfile { source: serde_yaml::Error },
    #[error("unexpected {kind} artifacts present: {}. Run `cargo run -p policy-meta --bin export-artifacts` to regenerate the canonical artifact set.", files.join(", "))]
    UnexpectedArtifacts {
        kind: ArtifactKind,
        files: Vec<String>,
    },
    #[error("{kind} artifacts out of sync: {}. Run `cargo run -p policy-meta --bin export-artifacts`.", files.join(", "))]
    Drift {
        kind: ArtifactKind,
        files: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportArtifactsCommand {
    pub(crate) check: bool,
    pub(crate) schema_dir: PathBuf,
    pub(crate) bindings_dir: PathBuf,
    pub(crate) profiles_dir: PathBuf,
}

impl ExportArtifactsCommand {
    pub(crate) fn defaults() -> Self {
        Self {
            check: false,
            schema_dir: default_schema_dir(),
            bindings_dir: default_bindings_dir(),
            profiles_dir: default_profiles_dir(),
        }
    }

    pub(crate) fn parse_args<I, S>(args: I) -> Result<Self, ExportArtifactsCommandError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut command = Self::defaults();
        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--check" => command.check = true,
                "--schema-dir" => {
                    let Some(path) = args.next() else {
                        return Err(ExportArtifactsCommandError::MissingValue {
                            flag: "--schema-dir",
                        });
                    };
                    command.schema_dir = PathBuf::from(path);
                }
                "--bindings-dir" => {
                    let Some(path) = args.next() else {
                        return Err(ExportArtifactsCommandError::MissingValue {
                            flag: "--bindings-dir",
                        });
                    };
                    command.bindings_dir = PathBuf::from(path);
                }
                "--profiles-dir" => {
                    let Some(path) = args.next() else {
                        return Err(ExportArtifactsCommandError::MissingValue {
                            flag: "--profiles-dir",
                        });
                    };
                    command.profiles_dir = PathBuf::from(path);
                }
                other => {
                    return Err(ExportArtifactsCommandError::UnknownArgument {
                        arg: other.to_string(),
                    });
                }
            }
        }
        Ok(command)
    }

    pub(crate) fn run(&self) -> Result<ExportArtifactsOutcome, ExportArtifactsCommandError> {
        if self.check {
            check_schema_dir(&self.schema_dir)?;
            check_typescript_bindings(&self.bindings_dir)?;
            check_profiles_dir(&self.profiles_dir)?;
            Ok(ExportArtifactsOutcome::Checked)
        } else {
            write_schema_dir(&self.schema_dir)?;
            write_typescript_bindings(&self.bindings_dir)?;
            write_profiles_dir(&self.profiles_dir)?;
            Ok(ExportArtifactsOutcome::Written {
                schema_dir: self.schema_dir.clone(),
                bindings_dir: self.bindings_dir.clone(),
                profiles_dir: self.profiles_dir.clone(),
            })
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExportArtifactsOutcome {
    Checked,
    Written {
        schema_dir: PathBuf,
        bindings_dir: PathBuf,
        profiles_dir: PathBuf,
    },
}

impl ExportArtifactsOutcome {
    pub(crate) fn success_message(&self) -> String {
        match self {
            Self::Checked => "all checked-in artifacts are in sync".to_string(),
            Self::Written {
                schema_dir,
                bindings_dir,
                profiles_dir,
            } => format!(
                "wrote checked-in artifacts to {}, {} and {}",
                schema_dir.display(),
                bindings_dir.display(),
                profiles_dir.display()
            ),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExportArtifactsCommandError {
    #[error("missing path after {flag}")]
    MissingValue { flag: &'static str },
    #[error("unknown argument: {arg}")]
    UnknownArgument { arg: String },
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
}

pub(crate) fn write_schema_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(output_dir).map_err(|source| ArtifactError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;
    prune_unexpected_artifacts(
        output_dir,
        schema_documents().iter().map(|(file_name, _)| *file_name),
    )?;
    for (file_name, schema) in schema_documents() {
        let path = output_dir.join(file_name);
        fs::write(&path, render_schema(&schema)?)
            .map_err(|source| ArtifactError::WriteFile { path, source })?;
    }
    Ok(())
}

pub(crate) fn check_schema_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    check_no_unexpected_artifacts(
        output_dir,
        schema_documents().iter().map(|(file_name, _)| *file_name),
        ArtifactKind::Schema,
    )?;
    let mut drift = Vec::<String>::new();

    for (file_name, expected) in schema_documents() {
        let path = output_dir.join(file_name);
        let contents = fs::read_to_string(&path).map_err(|source| ArtifactError::ReadFile {
            path: path.clone(),
            source,
        })?;
        let actual: serde_json::Value =
            serde_json::from_str(&contents).map_err(|source| ArtifactError::ParseJson {
                path: path.clone(),
                source,
            })?;

        if actual != expected {
            drift.push(file_name.to_string());
        }
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(ArtifactError::Drift {
            kind: ArtifactKind::Schema,
            files: drift,
        })
    }
}

pub(crate) fn write_typescript_bindings(output_dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(output_dir).map_err(|source| ArtifactError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;
    prune_unexpected_artifacts(output_dir, [POLICY_META_TYPES_FILE])?;
    let path = output_dir.join(POLICY_META_TYPES_FILE);
    fs::write(&path, policy_meta_typescript_bindings())
        .map_err(|source| ArtifactError::WriteFile { path, source })?;
    Ok(())
}

pub(crate) fn check_typescript_bindings(output_dir: &Path) -> Result<(), ArtifactError> {
    check_no_unexpected_artifacts(
        output_dir,
        [POLICY_META_TYPES_FILE],
        ArtifactKind::TypescriptBinding,
    )?;
    let path = output_dir.join(POLICY_META_TYPES_FILE);
    let actual = fs::read_to_string(&path).map_err(|source| ArtifactError::ReadFile {
        path: path.clone(),
        source,
    })?;
    let expected = policy_meta_typescript_bindings();

    if actual == expected {
        Ok(())
    } else {
        Err(ArtifactError::Drift {
            kind: ArtifactKind::TypescriptBinding,
            files: vec![POLICY_META_TYPES_FILE.to_string()],
        })
    }
}

pub(crate) fn write_profiles_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(output_dir).map_err(|source| ArtifactError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;
    prune_unexpected_artifacts(
        output_dir,
        profile_documents().iter().map(|(file_name, _)| *file_name),
    )?;
    for (file_name, profile) in profile_documents() {
        let path = output_dir.join(file_name);
        fs::write(&path, render_profile(&profile)?)
            .map_err(|source| ArtifactError::WriteFile { path, source })?;
    }
    Ok(())
}

pub(crate) fn check_profiles_dir(output_dir: &Path) -> Result<(), ArtifactError> {
    check_no_unexpected_artifacts(
        output_dir,
        profile_documents().iter().map(|(file_name, _)| *file_name),
        ArtifactKind::Profile,
    )?;
    let mut drift = Vec::<String>::new();

    for (file_name, expected) in profile_documents() {
        let path = output_dir.join(file_name);
        let actual = fs::read_to_string(&path).map_err(|source| ArtifactError::ReadFile {
            path: path.clone(),
            source,
        })?;
        if actual != render_profile(&expected)? {
            drift.push(file_name.to_string());
        }
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(ArtifactError::Drift {
            kind: ArtifactKind::Profile,
            files: drift,
        })
    }
}

fn render_schema(schema: &serde_json::Value) -> Result<String, ArtifactError> {
    let mut rendered = serde_json::to_string_pretty(schema)
        .map_err(|source| ArtifactError::RenderSchema { source })?;
    rendered.push('\n');
    Ok(rendered)
}

fn render_profile(profile: &PolicyProfileV1) -> Result<String, ArtifactError> {
    let rendered =
        serde_yaml::to_string(profile).map_err(|source| ArtifactError::RenderProfile { source })?;
    Ok(rendered
        .strip_prefix("---\n")
        .unwrap_or(rendered.as_str())
        .to_string())
}

fn default_schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
}

fn default_bindings_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings")
}

fn default_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("profiles")
}

fn prune_unexpected_artifacts<'a>(
    output_dir: &Path,
    expected_files: impl IntoIterator<Item = &'a str>,
) -> Result<(), ArtifactError> {
    let expected = expected_artifact_names(expected_files);
    for entry in fs::read_dir(output_dir).map_err(|source| ArtifactError::ReadDir {
        path: output_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ArtifactError::InspectDir {
            path: output_dir.to_path_buf(),
            source,
        })?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if expected.contains(file_name.as_ref()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| ArtifactError::InspectDir {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path)
                .map_err(|source| ArtifactError::RemoveArtifact { path, source })?;
        } else {
            fs::remove_file(&path)
                .map_err(|source| ArtifactError::RemoveArtifact { path, source })?;
        }
    }
    Ok(())
}

fn check_no_unexpected_artifacts<'a>(
    output_dir: &Path,
    expected_files: impl IntoIterator<Item = &'a str>,
    artifact_kind: ArtifactKind,
) -> Result<(), ArtifactError> {
    let expected = expected_artifact_names(expected_files);
    let mut unexpected = Vec::new();
    for entry in fs::read_dir(output_dir).map_err(|source| ArtifactError::ReadDir {
        path: output_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ArtifactError::InspectDir {
            path: output_dir.to_path_buf(),
            source,
        })?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !expected.contains(file_name.as_ref()) {
            unexpected.push(file_name.into_owned());
        }
    }
    unexpected.sort();
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(ArtifactError::UnexpectedArtifacts {
        kind: artifact_kind,
        files: unexpected,
    })
}

fn expected_artifact_names<'a>(
    expected_files: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<&'a str> {
    expected_files.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> std::io::Result<Self> {
            let path = env::temp_dir().join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            if path.exists() {
                fs::remove_dir_all(&path)?;
            }
            fs::create_dir_all(&path)?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn unique_tempdir(label: &str) -> TempDir {
        TempDir::new(label).expect("create tempdir")
    }

    #[test]
    fn check_schema_dir_rejects_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-schema-check");
        write_schema_dir(dir.path()).expect("write canonical schema dir");
        let stale = dir.path().join("stale-policy-meta.json");
        std::fs::write(&stale, "{}\n").expect("write stale schema artifact");

        let err =
            check_schema_dir(dir.path()).expect_err("stale schema artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::UnexpectedArtifacts { kind: ArtifactKind::Schema, ref files }
            if files == &vec!["stale-policy-meta.json".to_string()]
        ));
    }

    #[test]
    fn write_schema_dir_prunes_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-schema-write");
        let stale = dir.path().join("obsolete.json");
        std::fs::write(&stale, "{}\n").expect("write stale schema artifact");

        write_schema_dir(dir.path()).expect("write canonical schema dir");

        assert!(
            !stale.exists(),
            "regeneration should remove stale schema artifacts instead of leaving drift behind"
        );
    }

    #[test]
    fn check_typescript_bindings_rejects_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-bindings-check");
        write_typescript_bindings(dir.path()).expect("write canonical bindings dir");
        let stale = dir.path().join("obsolete.d.ts");
        std::fs::write(&stale, "// stale\n").expect("write stale bindings artifact");

        let err = check_typescript_bindings(dir.path())
            .expect_err("stale bindings artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::UnexpectedArtifacts { kind: ArtifactKind::TypescriptBinding, ref files }
            if files == &vec!["obsolete.d.ts".to_string()]
        ));
    }

    #[test]
    fn check_profiles_dir_rejects_stale_artifacts() {
        let dir = unique_tempdir("policy-meta-profiles-check");
        write_profiles_dir(dir.path()).expect("write canonical profiles dir");
        let stale = dir.path().join("obsolete.yaml");
        std::fs::write(&stale, "version: 1\n").expect("write stale profile artifact");

        let err =
            check_profiles_dir(dir.path()).expect_err("stale profile artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::UnexpectedArtifacts { kind: ArtifactKind::Profile, ref files }
            if files == &vec!["obsolete.yaml".to_string()]
        ));
    }

    #[test]
    fn check_schema_dir_reports_parse_failures_structurally() {
        let dir = unique_tempdir("policy-meta-schema-parse");
        write_schema_dir(dir.path()).expect("write canonical schema dir");
        let path = dir.path().join(POLICY_META_SCHEMA_FILE);
        std::fs::write(&path, "{not-json\n").expect("write invalid schema artifact");

        let err = check_schema_dir(dir.path()).expect_err("invalid json should fail check");
        assert!(matches!(
            err,
            ArtifactError::ParseJson { path: ref actual_path, .. } if actual_path == &path
        ));
    }

    #[test]
    fn check_typescript_bindings_reports_drift_structurally() {
        let dir = unique_tempdir("policy-meta-bindings-drift");
        write_typescript_bindings(dir.path()).expect("write canonical bindings dir");
        let path = dir.path().join(POLICY_META_TYPES_FILE);
        std::fs::write(&path, "// drifted\n").expect("write drifted bindings artifact");

        let err = check_typescript_bindings(dir.path())
            .expect_err("drifted bindings artifact should fail check");
        assert!(matches!(
            err,
            ArtifactError::Drift { kind: ArtifactKind::TypescriptBinding, ref files }
            if files == &vec![POLICY_META_TYPES_FILE.to_string()]
        ));
    }

    #[test]
    fn export_artifacts_command_rejects_missing_flag_value_structurally() {
        let err = ExportArtifactsCommand::parse_args(["--schema-dir"])
            .expect_err("missing value should fail");
        assert!(matches!(
            err,
            ExportArtifactsCommandError::MissingValue {
                flag: "--schema-dir"
            }
        ));
    }

    #[test]
    fn export_artifacts_command_rejects_unknown_argument_structurally() {
        let err = ExportArtifactsCommand::parse_args(["--wat"])
            .expect_err("unknown argument should fail");
        assert!(matches!(
            err,
            ExportArtifactsCommandError::UnknownArgument { ref arg } if arg == "--wat"
        ));
    }

    #[test]
    fn export_artifacts_command_surfaces_artifact_errors_without_erasure() {
        let tempdir = unique_tempdir("policy-meta-missing-root");
        let missing_root = tempdir.path().join("does-not-exist");
        let command = ExportArtifactsCommand {
            check: true,
            schema_dir: missing_root.clone(),
            bindings_dir: missing_root.clone(),
            profiles_dir: missing_root,
        };

        let err = command
            .run()
            .expect_err("missing artifact directory should fail");
        assert!(matches!(
            err,
            ExportArtifactsCommandError::Artifact(ArtifactError::ReadDir { .. })
        ));
    }
}
