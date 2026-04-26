#![forbid(unsafe_code)]

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use omne_artifact_install_primitives::{
    ArtifactDownloadCandidate, ArtifactDownloader, DownloadFileRequest,
    download_file_to_destination,
};
use omne_fs_primitives::{
    AtomicWriteOptions, write_file_atomically, write_file_atomically_from_reader,
};
use omne_integrity_primitives::{Sha256Digest, parse_sha256_user_input, verify_sha256_reader};
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use thiserror::Error;

const MIB: u64 = 1_048_576;
const WHISPER_CPP_HF_REPO: &str = "ggerganov/whisper.cpp";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelAssetCatalog {
    pub manifests: Vec<ModelManifest>,
    pub local_models: Vec<LocalModelRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelManifest {
    pub model_id: String,
    pub display_name: String,
    pub family: ModelFamily,
    pub format: ModelFormat,
    pub runtime_backend: Option<LocalModelRuntimeBackend>,
    pub version: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    #[serde(default)]
    pub checksum: Option<ModelChecksum>,
    pub license: Option<String>,
    pub sources: Vec<ModelSource>,
    pub capabilities: Vec<ModelCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChecksum {
    pub algorithm: ModelChecksumAlgorithm,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelChecksumAlgorithm {
    Sha1,
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModelFamily {
    Whisper,
    Embedding,
    TextGeneration,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModelFormat {
    WhisperCppGgml,
    Gguf,
    Onnx,
    Safetensors,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModelSource {
    HuggingFaceHub {
        repo_id: String,
        revision: Option<String>,
        file: String,
    },
    Https {
        url: String,
    },
    LocalFile {
        path: String,
    },
    Custom {
        id: String,
        uri: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModelCapability {
    SpeechTranscription,
    SpeechTranslation,
    SegmentTimestamps,
    WordTimestamps,
    VoiceActivityDetection,
    Language { code: String },
    Quantized { scheme: String },
    SpeakerDiarization,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelRef {
    pub model_id: String,
    pub path: String,
    pub format: ModelFormat,
    pub runtime_backend: Option<LocalModelRuntimeBackend>,
    pub sha256: Option<String>,
    #[serde(default)]
    pub checksum: Option<ModelChecksum>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInstallRequest {
    pub model_id: String,
    pub source: ModelSource,
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub expected_checksum: Option<ModelChecksum>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInstallProgress {
    pub model_id: String,
    pub status: ModelInstallStatus,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelInstallStatus {
    Pending,
    Downloading,
    Verifying,
    Installing,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LocalModelRuntimeBackend {
    WhisperCppSidecar,
    WhisperRs,
    CandleWhisper,
    Custom { id: String },
}

#[derive(Debug, Error)]
pub enum ModelStoreError {
    #[error("model manifest not found: {0}")]
    ManifestNotFound(String),
    #[error("invalid model manifest {model_id}: {message}")]
    InvalidManifest { model_id: String, message: String },
    #[error("model source is unsupported: {0}")]
    UnsupportedSource(String),
    #[error("model store io error while {op} ({path:?}): {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("model install failed: {0}")]
    Install(String),
    #[error("model checksum verification failed for {path:?}: {message}")]
    Checksum { path: PathBuf, message: String },
    #[error("model verification failed for {path:?}: {message}")]
    Verification { path: PathBuf, message: String },
    #[error("model metadata error ({path:?}): {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStore {
    root_dir: PathBuf,
}

impl ModelStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Result<Self, ModelStoreError> {
        let root_dir = root_dir.into();
        fs::create_dir_all(&root_dir).map_err(|source| ModelStoreError::Io {
            op: "create model store root",
            path: root_dir.clone(),
            source,
        })?;
        Ok(Self { root_dir })
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn list_local_models(&self) -> Result<Vec<LocalModelRef>, ModelStoreError> {
        let mut models = Vec::new();
        if !self.root_dir.exists() {
            return Ok(models);
        }

        let entries = fs::read_dir(&self.root_dir).map_err(|source| ModelStoreError::Io {
            op: "read model store root",
            path: self.root_dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ModelStoreError::Io {
                op: "read model store entry",
                path: self.root_dir.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| ModelStoreError::Io {
                op: "read model store entry metadata",
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            if let Some(model) = self.read_local_model_from_dir(&path)? {
                models.push(model);
            }
        }

        models.sort_by(|left, right| left.model_id.cmp(&right.model_id));
        Ok(models)
    }

    pub fn find_local_model(
        &self,
        model_id: &str,
    ) -> Result<Option<LocalModelRef>, ModelStoreError> {
        let model_dir = self.model_dir(model_id)?;
        if !model_dir.exists() {
            return Ok(None);
        }
        let metadata = fs::symlink_metadata(&model_dir).map_err(|source| ModelStoreError::Io {
            op: "read model directory metadata",
            path: model_dir.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ModelStoreError::InvalidManifest {
                model_id: model_id.to_string(),
                message: "model directory is not a regular directory".to_string(),
            });
        }
        self.read_local_model_from_dir(&model_dir)
    }

    pub async fn install_manifest<D>(
        &self,
        downloader: &D,
        manifest: &ModelManifest,
    ) -> Result<LocalModelRef, ModelStoreError>
    where
        D: ArtifactDownloader + ?Sized,
    {
        let destination = self.model_destination(manifest)?;
        if destination_has_regular_model_file(manifest, &destination)?
            && self.verify_model_file(manifest, &destination).is_ok()
        {
            let local_model = local_model_ref_from_manifest(manifest, &destination);
            self.write_local_model_metadata(&local_model)?;
            return Ok(local_model);
        }

        if let Some(source_path) = first_local_source(manifest) {
            self.install_from_local_file(manifest, source_path, &destination)?;
            let local_model = local_model_ref_from_manifest(manifest, &destination);
            self.write_local_model_metadata(&local_model)?;
            return Ok(local_model);
        }

        let candidates = download_candidates(manifest)?;
        if candidates.is_empty() {
            return Err(ModelStoreError::UnsupportedSource(
                manifest.model_id.clone(),
            ));
        }

        let asset_name = asset_name_for_manifest(manifest)?;
        let expected_sha256 = manifest_sha256_digest(manifest)?;
        let max_download_bytes = manifest
            .size_bytes
            .and_then(|bytes| bytes.checked_add(16 * MIB));
        let canonical_url = candidates[0].url.as_str();
        let request = DownloadFileRequest {
            canonical_url,
            destination: &destination,
            asset_name: &asset_name,
            expected_sha256: expected_sha256.as_ref(),
            max_download_bytes,
        };
        download_file_to_destination(downloader, &candidates, &request)
            .await
            .map_err(|error| ModelStoreError::Install(error.to_string()))?;

        self.verify_model_file(manifest, &destination)?;
        let local_model = local_model_ref_from_manifest(manifest, &destination);
        self.write_local_model_metadata(&local_model)?;
        Ok(local_model)
    }

    pub fn delete_model(&self, model_id: &str) -> Result<bool, ModelStoreError> {
        let model_dir = self.model_dir(model_id)?;
        if !model_dir.exists() {
            return Ok(false);
        }
        let metadata = fs::symlink_metadata(&model_dir).map_err(|source| ModelStoreError::Io {
            op: "read model directory metadata",
            path: model_dir.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ModelStoreError::InvalidManifest {
                model_id: model_id.to_string(),
                message: "model directory is not a regular directory".to_string(),
            });
        }
        fs::remove_dir_all(&model_dir).map_err(|source| ModelStoreError::Io {
            op: "delete model directory",
            path: model_dir,
            source,
        })?;
        Ok(true)
    }

    pub fn manifest_by_id(&self, model_id: &str) -> Option<ModelManifest> {
        whisper_cpp_builtin_model_manifests()
            .into_iter()
            .find(|manifest| manifest.model_id == model_id)
    }

    fn model_dir(&self, model_id: &str) -> Result<PathBuf, ModelStoreError> {
        let component = safe_component(model_id, "model id").map_err(|message| {
            ModelStoreError::InvalidManifest {
                model_id: model_id.to_string(),
                message,
            }
        })?;
        Ok(self.root_dir.join(component))
    }

    fn model_destination(&self, manifest: &ModelManifest) -> Result<PathBuf, ModelStoreError> {
        let model_dir = self.model_dir(&manifest.model_id)?;
        let asset_name = asset_name_for_manifest(manifest)?;
        Ok(model_dir.join(asset_name))
    }

    fn read_local_model_from_dir(
        &self,
        model_dir: &Path,
    ) -> Result<Option<LocalModelRef>, ModelStoreError> {
        let metadata_path = model_dir.join("model.json");
        match fs::symlink_metadata(&metadata_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(ModelStoreError::InvalidManifest {
                        model_id: model_dir
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("<unknown>")
                            .to_string(),
                        message: "model metadata is not a regular file".to_string(),
                    });
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ModelStoreError::Io {
                    op: "read model metadata file metadata",
                    path: metadata_path.clone(),
                    source,
                });
            }
        }

        if metadata_path.is_file() {
            let content =
                fs::read_to_string(&metadata_path).map_err(|source| ModelStoreError::Io {
                    op: "read model metadata",
                    path: metadata_path.clone(),
                    source,
                })?;
            let model = serde_json::from_str::<LocalModelRef>(&content).map_err(|source| {
                ModelStoreError::Metadata {
                    path: metadata_path.clone(),
                    source,
                }
            })?;
            if !local_model_path_is_regular_child(model_dir, &model) {
                return Err(ModelStoreError::InvalidManifest {
                    model_id: model.model_id.clone(),
                    message: "model metadata path must point to a regular file inside the model directory"
                        .to_string(),
                });
            }
            return Ok(Some(model));
        }

        let Some(model_id) = model_dir.file_name().and_then(|value| value.to_str()) else {
            return Ok(None);
        };
        let entries = fs::read_dir(model_dir).map_err(|source| ModelStoreError::Io {
            op: "read model directory",
            path: model_dir.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ModelStoreError::Io {
                op: "read model file entry",
                path: model_dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| ModelStoreError::Io {
                op: "read model file metadata",
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                continue;
            }
            if is_legacy_whisper_ggml_file(&path) {
                return Ok(Some(LocalModelRef {
                    model_id: model_id.to_string(),
                    path: path.display().to_string(),
                    format: ModelFormat::WhisperCppGgml,
                    runtime_backend: Some(LocalModelRuntimeBackend::WhisperRs),
                    sha256: None,
                    checksum: None,
                }));
            }
        }
        Ok(None)
    }

    fn install_from_local_file(
        &self,
        manifest: &ModelManifest,
        source_path: &Path,
        destination: &Path,
    ) -> Result<(), ModelStoreError> {
        let mut reader = File::open(source_path).map_err(|source| ModelStoreError::Io {
            op: "open local model source",
            path: source_path.to_path_buf(),
            source,
        })?;
        write_file_atomically_from_reader(&mut reader, destination, &model_file_write_options())
            .map_err(|source| ModelStoreError::Io {
                op: "install local model file",
                path: destination.to_path_buf(),
                source: io::Error::other(source),
            })?;
        self.verify_model_file(manifest, destination)
    }

    fn write_local_model_metadata(
        &self,
        local_model: &LocalModelRef,
    ) -> Result<(), ModelStoreError> {
        let metadata_path = Path::new(&local_model.path)
            .parent()
            .ok_or_else(|| ModelStoreError::InvalidManifest {
                model_id: local_model.model_id.clone(),
                message: "local model path has no parent directory".to_string(),
            })?
            .join("model.json");
        let bytes =
            serde_json::to_vec_pretty(local_model).map_err(|source| ModelStoreError::Metadata {
                path: metadata_path.clone(),
                source,
            })?;
        write_file_atomically(&bytes, &metadata_path, &metadata_write_options()).map_err(|source| {
            ModelStoreError::Io {
                op: "write model metadata",
                path: metadata_path,
                source: io::Error::other(source),
            }
        })
    }

    fn verify_model_file(
        &self,
        manifest: &ModelManifest,
        path: &Path,
    ) -> Result<(), ModelStoreError> {
        require_regular_model_file(manifest, path)?;
        verify_model_size(manifest, path)?;
        if let Some(expected) = manifest_sha256_digest(manifest)? {
            let mut file = File::open(path).map_err(|source| ModelStoreError::Io {
                op: "open model for sha256 verification",
                path: path.to_path_buf(),
                source,
            })?;
            verify_sha256_reader(&mut file, &expected).map_err(|error| {
                ModelStoreError::Checksum {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                }
            })?;
        }

        if let Some(checksum) = &manifest.checksum {
            verify_checksum(path, checksum)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperCppModelSpec {
    pub name: &'static str,
    pub disk_size_bytes: u64,
    pub sha1: &'static str,
    pub english_only: bool,
    pub quantization: Option<&'static str>,
    pub diarization: bool,
}

impl WhisperCppModelSpec {
    pub fn model_id(self) -> String {
        whisper_cpp_model_id(self.name)
    }

    pub fn file_name(self) -> String {
        format!("ggml-{}.bin", self.name)
    }

    pub fn display_name(self) -> String {
        let mut label = format!("Whisper {}", self.name);
        if self.quantization.is_some() {
            label.push_str(" quantized");
        }
        if self.diarization {
            label.push_str(" tinydiarize");
        }
        label
    }
}

pub const WHISPER_CPP_MODEL_SPECS: &[WhisperCppModelSpec] = &[
    WhisperCppModelSpec {
        name: "tiny",
        disk_size_bytes: 77_691_713,
        sha1: "bd577a113a864445d4c299885e0cb97d4ba92b5f",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "tiny-q5_1",
        disk_size_bytes: 31 * MIB,
        sha1: "2827a03e495b1ed3048ef28a6a4620537db4ee51",
        english_only: false,
        quantization: Some("q5_1"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "tiny-q8_0",
        disk_size_bytes: 42 * MIB,
        sha1: "19e8118f6652a650569f5a949d962154e01571d9",
        english_only: false,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "tiny.en",
        disk_size_bytes: 77_704_715,
        sha1: "c78c86eb1a8faa21b369bcd33207cc90d64ae9df",
        english_only: true,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "tiny.en-q5_1",
        disk_size_bytes: 31 * MIB,
        sha1: "3fb92ec865cbbc769f08137f22470d6b66e071b6",
        english_only: true,
        quantization: Some("q5_1"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "tiny.en-q8_0",
        disk_size_bytes: 42 * MIB,
        sha1: "802d6668e7d411123e672abe4cb6c18f12306abb",
        english_only: true,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "base",
        disk_size_bytes: 147_951_465,
        sha1: "465707469ff3a37a2b9b8d8f89f2f99de7299dac",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "base-q5_1",
        disk_size_bytes: 57 * MIB,
        sha1: "a3733eda680ef76256db5fc5dd9de8629e62c5e7",
        english_only: false,
        quantization: Some("q5_1"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "base-q8_0",
        disk_size_bytes: 78 * MIB,
        sha1: "7bb89bb49ed6955013b166f1b6a6c04584a20fbe",
        english_only: false,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "base.en",
        disk_size_bytes: 147_964_211,
        sha1: "137c40403d78fd54d454da0f9bd998f78703390c",
        english_only: true,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "base.en-q5_1",
        disk_size_bytes: 57 * MIB,
        sha1: "d26d7ce5a1b6e57bea5d0431b9c20ae49423c94a",
        english_only: true,
        quantization: Some("q5_1"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "base.en-q8_0",
        disk_size_bytes: 78 * MIB,
        sha1: "bb1574182e9b924452bf0cd1510ac034d323e948",
        english_only: true,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small",
        disk_size_bytes: 487_601_967,
        sha1: "55356645c2b361a969dfd0ef2c5a50d530afd8d5",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small-q5_1",
        disk_size_bytes: 181 * MIB,
        sha1: "6fe57ddcfdd1c6b07cdcc73aaf620810ce5fc771",
        english_only: false,
        quantization: Some("q5_1"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small-q8_0",
        disk_size_bytes: 252 * MIB,
        sha1: "bcad8a2083f4e53d648d586b7dbc0cd673d8afad",
        english_only: false,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small.en",
        disk_size_bytes: 487_614_201,
        sha1: "db8a495a91d927739e50b3fc1cc4c6b8f6c2d022",
        english_only: true,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small.en-q5_1",
        disk_size_bytes: 181 * MIB,
        sha1: "20f54878d608f94e4a8ee3ae56016571d47cba34",
        english_only: true,
        quantization: Some("q5_1"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small.en-q8_0",
        disk_size_bytes: 252 * MIB,
        sha1: "9d75ff4ccfa0a8217870d7405cf8cef0a5579852",
        english_only: true,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "small.en-tdrz",
        disk_size_bytes: 465 * MIB,
        sha1: "b6c6e7e89af1a35c08e6de56b66ca6a02a2fdfa1",
        english_only: true,
        quantization: None,
        diarization: true,
    },
    WhisperCppModelSpec {
        name: "medium",
        disk_size_bytes: 1_533_763_059,
        sha1: "fd9727b6e1217c2f614f9b698455c4ffd82463b4",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "medium-q5_0",
        disk_size_bytes: 514 * MIB,
        sha1: "7718d4c1ec62ca96998f058114db98236937490e",
        english_only: false,
        quantization: Some("q5_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "medium-q8_0",
        disk_size_bytes: 785 * MIB,
        sha1: "e66645948aff4bebbec71b3485c576f3d63af5d6",
        english_only: false,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "medium.en",
        disk_size_bytes: 1_533_774_781,
        sha1: "8c30f0e44ce9560643ebd10bbe50cd20eafd3723",
        english_only: true,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "medium.en-q5_0",
        disk_size_bytes: 514 * MIB,
        sha1: "bb3b5281bddd61605d6fc76bc5b92d8f20284c3b",
        english_only: true,
        quantization: Some("q5_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "medium.en-q8_0",
        disk_size_bytes: 785 * MIB,
        sha1: "b1cf48c12c807e14881f634fb7b6c6ca867f6b38",
        english_only: true,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v1",
        disk_size_bytes: 3_094_623_691,
        sha1: "b1caaf735c4cc1429223d5a74f0f4d0b9b59a299",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v2",
        disk_size_bytes: 3_094_623_691,
        sha1: "0f4c8e34f21cf1a914c59d8b3ce882345ad349d6",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v2-q5_0",
        disk_size_bytes: 1_080_732_091,
        sha1: "00e39f2196344e901b3a2bd5814807a769bd1630",
        english_only: false,
        quantization: Some("q5_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v2-q8_0",
        disk_size_bytes: 1_536 * MIB,
        sha1: "da97d6ca8f8ffbeeb5fd147f79010eeea194ba38",
        english_only: false,
        quantization: Some("q8_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v3",
        disk_size_bytes: 3_095_033_483,
        sha1: "ad82bf6a9043ceed055076d0fd39f5f186ff8062",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v3-q5_0",
        disk_size_bytes: 1_081_140_203,
        sha1: "e6e2ed78495d403bef4b7cff42ef4aaadcfea8de",
        english_only: false,
        quantization: Some("q5_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v3-turbo",
        disk_size_bytes: 1_624_555_275,
        sha1: "4af2b29d7ec73d781377bfd1758ca957a807e941",
        english_only: false,
        quantization: None,
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v3-turbo-q5_0",
        disk_size_bytes: 574_041_195,
        sha1: "e050f7970618a659205450ad97eb95a18d69c9ee",
        english_only: false,
        quantization: Some("q5_0"),
        diarization: false,
    },
    WhisperCppModelSpec {
        name: "large-v3-turbo-q8_0",
        disk_size_bytes: 834 * MIB,
        sha1: "01bf15bedffe9f39d65c1b6ff9b687ea91f59e0e",
        english_only: false,
        quantization: Some("q8_0"),
        diarization: false,
    },
];

pub fn local_whisper_compat_manifest() -> ModelManifest {
    ModelManifest {
        model_id: "local-whisper".to_string(),
        display_name: "User-supplied Whisper GGML model".to_string(),
        family: ModelFamily::Whisper,
        format: ModelFormat::WhisperCppGgml,
        runtime_backend: Some(LocalModelRuntimeBackend::WhisperRs),
        version: None,
        size_bytes: None,
        sha256: None,
        checksum: None,
        license: None,
        sources: vec![ModelSource::LocalFile {
            path: "/path/to/ggml-model.bin".to_string(),
        }],
        capabilities: whisper_model_capabilities(false, None, false),
    }
}

pub fn whisper_cpp_model_manifests() -> Vec<ModelManifest> {
    WHISPER_CPP_MODEL_SPECS
        .iter()
        .copied()
        .map(whisper_cpp_manifest)
        .collect()
}

pub fn whisper_cpp_builtin_model_manifests() -> Vec<ModelManifest> {
    let mut manifests = Vec::with_capacity(WHISPER_CPP_MODEL_SPECS.len() + 1);
    manifests.push(local_whisper_compat_manifest());
    manifests.extend(whisper_cpp_model_manifests());
    manifests
}

pub fn whisper_cpp_manifest(spec: WhisperCppModelSpec) -> ModelManifest {
    ModelManifest {
        model_id: spec.model_id(),
        display_name: spec.display_name(),
        family: ModelFamily::Whisper,
        format: ModelFormat::WhisperCppGgml,
        runtime_backend: Some(LocalModelRuntimeBackend::WhisperRs),
        version: Some("whisper.cpp official ggml".to_string()),
        size_bytes: Some(spec.disk_size_bytes),
        sha256: None,
        checksum: Some(ModelChecksum {
            algorithm: ModelChecksumAlgorithm::Sha1,
            value: spec.sha1.to_string(),
        }),
        license: Some("MIT".to_string()),
        sources: vec![ModelSource::HuggingFaceHub {
            repo_id: WHISPER_CPP_HF_REPO.to_string(),
            revision: Some("main".to_string()),
            file: spec.file_name(),
        }],
        capabilities: whisper_model_capabilities(
            spec.english_only,
            spec.quantization,
            spec.diarization,
        ),
    }
}

pub fn whisper_cpp_model_id(name: &str) -> String {
    format!("whisper-{name}")
}

pub fn infer_whisper_cpp_model_id(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_string_lossy();
    let name = file_name
        .strip_prefix("ggml-")
        .and_then(|value| value.strip_suffix(".bin"))?;

    WHISPER_CPP_MODEL_SPECS
        .iter()
        .any(|spec| spec.name == name)
        .then(|| whisper_cpp_model_id(name))
}

pub fn local_model_ref_from_whisper_path(
    path: &Path,
    runtime_backend: LocalModelRuntimeBackend,
) -> LocalModelRef {
    LocalModelRef {
        model_id: infer_whisper_cpp_model_id(path).unwrap_or_else(|| "local-whisper".to_string()),
        path: path.display().to_string(),
        format: ModelFormat::WhisperCppGgml,
        runtime_backend: Some(runtime_backend),
        sha256: None,
        checksum: None,
    }
}

fn is_legacy_whisper_ggml_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|file_name| file_name.starts_with("ggml-") && file_name.ends_with(".bin"))
}

fn local_model_path_is_regular_child(model_dir: &Path, model: &LocalModelRef) -> bool {
    let path = Path::new(&model.path);
    if path.parent() != Some(model_dir) {
        return false;
    }
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
}

fn destination_has_regular_model_file(
    manifest: &ModelManifest,
    path: &Path,
) -> Result<bool, ModelStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ModelStoreError::InvalidManifest {
                    model_id: manifest.model_id.clone(),
                    message: "model destination is not a regular file".to_string(),
                });
            }
            Ok(true)
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ModelStoreError::Io {
            op: "read model file metadata",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn require_regular_model_file(
    manifest: &ModelManifest,
    path: &Path,
) -> Result<(), ModelStoreError> {
    if destination_has_regular_model_file(manifest, path)? {
        return Ok(());
    }
    Err(ModelStoreError::InvalidManifest {
        model_id: manifest.model_id.clone(),
        message: "model file does not exist".to_string(),
    })
}

fn verify_model_size(manifest: &ModelManifest, path: &Path) -> Result<(), ModelStoreError> {
    let Some(expected_size) = manifest.size_bytes else {
        return Ok(());
    };
    let actual_size = fs::symlink_metadata(path)
        .map_err(|source| ModelStoreError::Io {
            op: "read model file size",
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if actual_size == expected_size {
        return Ok(());
    }
    Err(ModelStoreError::Verification {
        path: path.to_path_buf(),
        message: format!("size mismatch: expected {expected_size} bytes, got {actual_size}"),
    })
}

fn local_model_ref_from_manifest(manifest: &ModelManifest, path: &Path) -> LocalModelRef {
    LocalModelRef {
        model_id: manifest.model_id.clone(),
        path: path.display().to_string(),
        format: manifest.format.clone(),
        runtime_backend: manifest.runtime_backend.clone(),
        sha256: manifest.sha256.clone(),
        checksum: manifest.checksum.clone(),
    }
}

fn first_local_source(manifest: &ModelManifest) -> Option<&Path> {
    manifest.sources.iter().find_map(|source| match source {
        ModelSource::LocalFile { path } => Some(Path::new(path)),
        _ => None,
    })
}

fn download_candidates(
    manifest: &ModelManifest,
) -> Result<Vec<ArtifactDownloadCandidate>, ModelStoreError> {
    manifest
        .sources
        .iter()
        .filter_map(|source| match source {
            ModelSource::HuggingFaceHub {
                repo_id,
                revision,
                file,
            } => Some(
                hugging_face_resolve_url(repo_id, revision.as_deref(), file).map(|url| {
                    ArtifactDownloadCandidate {
                        url,
                        source_label: "hugging-face-hub".to_string(),
                    }
                }),
            ),
            ModelSource::Https { url } => Some(Ok(ArtifactDownloadCandidate {
                url: url.clone(),
                source_label: "https".to_string(),
            })),
            ModelSource::LocalFile { .. } | ModelSource::Custom { .. } => None,
        })
        .collect()
}

fn hugging_face_resolve_url(
    repo_id: &str,
    revision: Option<&str>,
    file: &str,
) -> Result<String, ModelStoreError> {
    let repo_id = repo_id.trim();
    let revision = revision.unwrap_or("main").trim();
    let file = file.trim();
    if repo_id.is_empty() || revision.is_empty() || file.is_empty() {
        return Err(ModelStoreError::InvalidManifest {
            model_id: repo_id.to_string(),
            message: "Hugging Face source requires repo_id, revision and file".to_string(),
        });
    }
    Ok(format!(
        "https://huggingface.co/{repo_id}/resolve/{revision}/{file}"
    ))
}

fn asset_name_for_manifest(manifest: &ModelManifest) -> Result<String, ModelStoreError> {
    manifest
        .sources
        .iter()
        .find_map(asset_name_for_source)
        .ok_or_else(|| ModelStoreError::InvalidManifest {
            model_id: manifest.model_id.clone(),
            message: "model manifest has no installable asset source".to_string(),
        })
        .and_then(|asset_name| {
            safe_component(&asset_name, "asset name").map_err(|message| {
                ModelStoreError::InvalidManifest {
                    model_id: manifest.model_id.clone(),
                    message,
                }
            })
        })
}

fn asset_name_for_source(source: &ModelSource) -> Option<String> {
    match source {
        ModelSource::HuggingFaceHub { file, .. } => path_leaf(file),
        ModelSource::Https { url } => url
            .split('?')
            .next()
            .and_then(path_leaf)
            .filter(|value| !value.is_empty()),
        ModelSource::LocalFile { path } => path_leaf(path),
        ModelSource::Custom { .. } => None,
    }
}

fn path_leaf(raw: &str) -> Option<String> {
    raw.rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn safe_component(raw: &str, label: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() || value == "." || value == ".." {
        return Err(format!("{label} is empty or reserved"));
    }
    if value.contains('/') || value.contains('\\') || value.contains('\0') {
        return Err(format!("{label} must be a single path component"));
    }
    Ok(value.to_string())
}

fn manifest_sha256_digest(
    manifest: &ModelManifest,
) -> Result<Option<Sha256Digest>, ModelStoreError> {
    let Some(raw) = manifest.sha256.as_deref() else {
        return Ok(None);
    };
    parse_sha256_user_input(raw)
        .map(Some)
        .ok_or_else(|| ModelStoreError::InvalidManifest {
            model_id: manifest.model_id.clone(),
            message: "invalid sha256 digest".to_string(),
        })
}

fn verify_checksum(path: &Path, checksum: &ModelChecksum) -> Result<(), ModelStoreError> {
    match checksum.algorithm {
        ModelChecksumAlgorithm::Sha1 => {
            let actual = sha1_file(path)?;
            let expected = checksum.value.trim().to_ascii_lowercase();
            if actual != expected {
                return Err(ModelStoreError::Checksum {
                    path: path.to_path_buf(),
                    message: format!("checksum mismatch: expected {expected}, got {actual}"),
                });
            }
        }
        ModelChecksumAlgorithm::Sha256 => {
            let expected = parse_sha256_user_input(&checksum.value).ok_or_else(|| {
                ModelStoreError::Checksum {
                    path: path.to_path_buf(),
                    message: "invalid sha256 digest".to_string(),
                }
            })?;
            let mut file = File::open(path).map_err(|source| ModelStoreError::Io {
                op: "open model for checksum verification",
                path: path.to_path_buf(),
                source,
            })?;
            verify_sha256_reader(&mut file, &expected).map_err(|error| {
                ModelStoreError::Checksum {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                }
            })?;
        }
    }
    Ok(())
}

fn sha1_file(path: &Path) -> Result<String, ModelStoreError> {
    let mut file = File::open(path).map_err(|source| ModelStoreError::Io {
        op: "open model for sha1 verification",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| ModelStoreError::Io {
                op: "read model for sha1 verification",
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn model_file_write_options() -> AtomicWriteOptions {
    AtomicWriteOptions {
        require_non_empty: true,
        ..AtomicWriteOptions::default()
    }
}

fn metadata_write_options() -> AtomicWriteOptions {
    AtomicWriteOptions {
        require_non_empty: true,
        ..AtomicWriteOptions::default()
    }
}

fn whisper_model_capabilities(
    english_only: bool,
    quantization: Option<&str>,
    diarization: bool,
) -> Vec<ModelCapability> {
    let mut capabilities = vec![ModelCapability::SpeechTranscription];
    if !english_only {
        capabilities.push(ModelCapability::SpeechTranslation);
    }
    capabilities.push(ModelCapability::Language {
        code: if english_only { "en" } else { "multi" }.to_string(),
    });
    if let Some(scheme) = quantization {
        capabilities.push(ModelCapability::Quantized {
            scheme: scheme.to_string(),
        });
    }
    if diarization {
        capabilities.push(ModelCapability::SpeakerDiarization);
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        path::{Path, PathBuf},
    };

    use omne_artifact_install_primitives::{ArtifactDownloader, ArtifactInstallError};
    use omne_integrity_primitives::hash_sha256;
    use tempfile::TempDir;

    use super::{
        LocalModelRuntimeBackend, ModelCapability, ModelChecksumAlgorithm, ModelFamily,
        ModelFormat, ModelInstallProgress, ModelInstallStatus, ModelManifest, ModelSource,
        ModelStore, ModelStoreError, WHISPER_CPP_MODEL_SPECS, infer_whisper_cpp_model_id,
        local_model_ref_from_manifest, local_model_ref_from_whisper_path,
        whisper_cpp_builtin_model_manifests, whisper_cpp_manifest, whisper_cpp_model_id,
    };

    #[test]
    fn model_manifest_serializes_stable_sources_and_capabilities() {
        let manifest = ModelManifest {
            model_id: "whisper-small-q5".to_string(),
            display_name: "Whisper Small Q5".to_string(),
            family: ModelFamily::Whisper,
            format: ModelFormat::Gguf,
            runtime_backend: Some(LocalModelRuntimeBackend::WhisperRs),
            version: Some("v1".to_string()),
            size_bytes: Some(512),
            sha256: Some("sha256:abc".to_string()),
            checksum: None,
            license: Some("MIT".to_string()),
            sources: vec![ModelSource::HuggingFaceHub {
                repo_id: "org/model".to_string(),
                revision: Some("main".to_string()),
                file: "model.gguf".to_string(),
            }],
            capabilities: vec![
                ModelCapability::SpeechTranscription,
                ModelCapability::Language {
                    code: "zh".to_string(),
                },
            ],
        };

        let value = serde_json::to_value(manifest).expect("serialize manifest");

        assert_eq!(value["modelId"], "whisper-small-q5");
        assert_eq!(value["family"]["kind"], "whisper");
        assert_eq!(value["runtimeBackend"]["kind"], "whisperRs");
        assert_eq!(value["sources"][0]["kind"], "huggingFaceHub");
        assert_eq!(value["sources"][0]["repoId"], "org/model");
        assert_eq!(value["capabilities"][1]["code"], "zh");
    }

    #[test]
    fn model_manifest_json_round_trips_and_ignores_unknown_fields() {
        let raw = serde_json::json!({
            "modelId": "whisper-fixture",
            "displayName": "Whisper fixture",
            "family": {"kind": "whisper", "futureField": true},
            "format": {"kind": "whisperCppGgml", "futureField": true},
            "runtimeBackend": {"kind": "whisperRs", "futureField": true},
            "version": "test",
            "sizeBytes": 1024,
            "sha256": null,
            "checksum": null,
            "license": "MIT",
            "sources": [{
                "kind": "https",
                "url": "https://models.example.invalid/ggml-fixture.bin",
                "futureField": true
            }],
            "capabilities": [{"kind": "speechTranscription", "futureField": true}],
            "futureField": "ignored"
        });

        let manifest: ModelManifest =
            serde_json::from_value(raw).expect("deserialize model manifest");

        assert_eq!(manifest.model_id, "whisper-fixture");
        assert_eq!(
            manifest.runtime_backend,
            Some(LocalModelRuntimeBackend::WhisperRs)
        );
        assert_eq!(manifest.sources.len(), 1);
        assert_eq!(
            manifest.capabilities,
            vec![ModelCapability::SpeechTranscription]
        );
    }

    #[test]
    fn install_progress_uses_stable_wire_names() {
        let progress = ModelInstallProgress {
            model_id: "whisper-small-q5".to_string(),
            status: ModelInstallStatus::Downloading,
            downloaded_bytes: Some(32),
            total_bytes: Some(64),
            message: None,
        };

        let value = serde_json::to_value(progress).expect("serialize progress");

        assert_eq!(value["status"], "downloading");
        assert_eq!(value["downloadedBytes"], 32);
        assert_eq!(value["totalBytes"], 64);
    }

    #[test]
    fn official_whisper_catalog_exposes_all_builtin_manifests() {
        let manifests = whisper_cpp_builtin_model_manifests();
        let model_ids = manifests
            .iter()
            .map(|manifest| manifest.model_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(manifests.len(), WHISPER_CPP_MODEL_SPECS.len() + 1);
        assert!(model_ids.contains(&"local-whisper"));
        assert!(model_ids.contains(&"whisper-tiny-q5_1"));
        assert!(model_ids.contains(&"whisper-large-v3-turbo-q8_0"));
    }

    #[test]
    fn official_whisper_manifest_keeps_sha1_checksum_and_hf_source() {
        let tiny = WHISPER_CPP_MODEL_SPECS
            .iter()
            .copied()
            .find(|spec| spec.name == "tiny")
            .expect("tiny spec");
        let manifest = whisper_cpp_manifest(tiny);
        let checksum = manifest.checksum.expect("checksum");

        assert_eq!(manifest.model_id, "whisper-tiny");
        assert!(manifest.sha256.is_none());
        assert_eq!(checksum.algorithm, ModelChecksumAlgorithm::Sha1);
        assert_eq!(checksum.value, "bd577a113a864445d4c299885e0cb97d4ba92b5f");
        assert_eq!(
            manifest.sources,
            vec![ModelSource::HuggingFaceHub {
                repo_id: "ggerganov/whisper.cpp".to_string(),
                revision: Some("main".to_string()),
                file: "ggml-tiny.bin".to_string(),
            }]
        );
    }

    #[test]
    fn official_whisper_manifest_marks_quantized_and_diarization_capabilities() {
        let quantized = WHISPER_CPP_MODEL_SPECS
            .iter()
            .copied()
            .find(|spec| spec.name == "large-v3-turbo-q8_0")
            .map(whisper_cpp_manifest)
            .expect("quantized spec");
        assert!(quantized
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModelCapability::Quantized { scheme } if scheme == "q8_0")));

        let diarization = WHISPER_CPP_MODEL_SPECS
            .iter()
            .copied()
            .find(|spec| spec.name == "small.en-tdrz")
            .map(whisper_cpp_manifest)
            .expect("diarization spec");
        assert!(
            diarization
                .capabilities
                .iter()
                .any(|capability| matches!(capability, ModelCapability::SpeakerDiarization))
        );
    }

    #[test]
    fn infers_known_model_ids_from_ggml_file_names() {
        assert_eq!(
            infer_whisper_cpp_model_id(Path::new("/models/ggml-large-v3-turbo-q5_0.bin"))
                .as_deref(),
            Some("whisper-large-v3-turbo-q5_0")
        );
        assert!(infer_whisper_cpp_model_id(Path::new("/models/custom.bin")).is_none());
    }

    #[test]
    fn local_model_ref_infers_official_ids_but_keeps_custom_compatibility() {
        let known = local_model_ref_from_whisper_path(
            Path::new("/models/ggml-base.en-q8_0.bin"),
            LocalModelRuntimeBackend::WhisperRs,
        );
        let custom = local_model_ref_from_whisper_path(
            Path::new("/models/my-custom.bin"),
            LocalModelRuntimeBackend::WhisperCppSidecar,
        );

        assert_eq!(known.model_id, whisper_cpp_model_id("base.en-q8_0"));
        assert_eq!(custom.model_id, "local-whisper");
        assert_eq!(
            serde_json::to_value(known).expect("serialize known")["runtimeBackend"]["kind"],
            "whisperRs"
        );
    }

    #[tokio::test]
    async fn model_store_installs_local_file_and_lists_metadata() {
        let temp = TempDir::new().expect("temp dir");
        let source = temp.path().join("ggml-fixture.bin");
        std::fs::write(&source, b"fixture-model").expect("write source");
        let manifest = fixture_manifest(ModelSource::LocalFile {
            path: source.display().to_string(),
        });
        let store = ModelStore::new(temp.path().join("models")).expect("store");

        let installed = store
            .install_manifest(&NoopDownloader, &manifest)
            .await
            .expect("install local model");

        assert_eq!(installed.model_id, "whisper-fixture");
        assert!(Path::new(&installed.path).is_file());
        assert_eq!(
            store.list_local_models().expect("list models"),
            vec![installed]
        );
    }

    #[test]
    fn model_store_rejects_metadata_path_outside_model_directory() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("whisper-fixture");
        let outside = temp.path().join("ggml-outside.bin");
        std::fs::create_dir_all(&model_dir).expect("create model dir");
        std::fs::write(&outside, b"outside").expect("write outside model");
        let model = local_model_ref_from_manifest(
            &fixture_manifest(ModelSource::LocalFile {
                path: outside.display().to_string(),
            }),
            &outside,
        );
        std::fs::write(
            model_dir.join("model.json"),
            serde_json::to_vec_pretty(&model).expect("serialize metadata"),
        )
        .expect("write metadata");
        let store = ModelStore::new(&root).expect("store");

        let error = store
            .find_local_model("whisper-fixture")
            .expect_err("metadata path outside model directory should fail");

        assert!(matches!(error, ModelStoreError::InvalidManifest { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn model_store_rejects_symlinked_metadata_file() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("linked-metadata");
        let target = temp.path().join("model.json");
        std::fs::create_dir_all(&model_dir).expect("create model dir");
        std::fs::write(&target, b"{}").expect("write target metadata");
        symlink(&target, model_dir.join("model.json")).expect("create metadata symlink");
        let store = ModelStore::new(&root).expect("store");

        let error = store
            .find_local_model("linked-metadata")
            .expect_err("symlinked metadata should fail");

        assert!(matches!(error, ModelStoreError::InvalidManifest { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_store_rejects_existing_symlink_destination() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("whisper-fixture");
        let source = temp.path().join("ggml-fixture.bin");
        let outside = temp.path().join("outside.bin");
        std::fs::create_dir_all(&model_dir).expect("create model dir");
        std::fs::write(&source, b"fixture-model").expect("write source");
        std::fs::write(&outside, b"outside").expect("write outside");
        symlink(&outside, model_dir.join("ggml-fixture.bin")).expect("create model symlink");
        let manifest = fixture_manifest(ModelSource::LocalFile {
            path: source.display().to_string(),
        });
        let store = ModelStore::new(&root).expect("store");

        let error = store
            .install_manifest(&NoopDownloader, &manifest)
            .await
            .expect_err("symlinked destination should fail");

        assert!(matches!(error, ModelStoreError::InvalidManifest { .. }));
    }

    #[tokio::test]
    async fn model_store_rejects_local_file_size_mismatch() {
        let temp = TempDir::new().expect("temp dir");
        let source = temp.path().join("ggml-fixture.bin");
        std::fs::write(&source, b"fixture-model").expect("write source");
        let manifest = ModelManifest {
            size_bytes: Some(1),
            ..fixture_manifest(ModelSource::LocalFile {
                path: source.display().to_string(),
            })
        };
        let store = ModelStore::new(temp.path().join("models")).expect("store");

        let error = store
            .install_manifest(&NoopDownloader, &manifest)
            .await
            .expect_err("size mismatch should fail");

        assert!(matches!(error, ModelStoreError::Verification { .. }));
    }

    #[tokio::test]
    async fn model_store_replaces_existing_destination_with_size_mismatch() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("whisper-fixture");
        let destination = model_dir.join("ggml-fixture.bin");
        std::fs::create_dir_all(&model_dir).expect("create model dir");
        std::fs::write(&destination, b"bad").expect("write existing wrong model");
        let bytes = b"right-size".to_vec();
        let manifest = ModelManifest {
            size_bytes: Some(bytes.len() as u64),
            sources: vec![ModelSource::Https {
                url: "https://models.example.invalid/ggml-fixture.bin".to_string(),
            }],
            ..fixture_manifest(ModelSource::Custom {
                id: "unused".to_string(),
                uri: "unused".to_string(),
            })
        };
        let store = ModelStore::new(&root).expect("store");

        let installed = store
            .install_manifest(
                &BytesDownloader {
                    bytes: bytes.clone(),
                },
                &manifest,
            )
            .await
            .expect("replace wrong-size model");

        assert_eq!(Path::new(&installed.path), destination.as_path());
        assert_eq!(std::fs::read(destination).expect("read installed"), bytes);
    }

    #[tokio::test]
    async fn model_store_downloads_with_runtime_artifact_downloader() {
        let temp = TempDir::new().expect("temp dir");
        let bytes = b"downloaded-model".to_vec();
        let sha256 = hash_sha256(&bytes).to_string();
        let manifest = ModelManifest {
            sha256: Some(sha256.clone()),
            checksum: Some(super::ModelChecksum {
                algorithm: ModelChecksumAlgorithm::Sha256,
                value: sha256,
            }),
            sources: vec![ModelSource::Https {
                url: "https://models.example.invalid/ggml-fixture.bin".to_string(),
            }],
            ..fixture_manifest(ModelSource::Custom {
                id: "unused".to_string(),
                uri: "unused".to_string(),
            })
        };
        let store = ModelStore::new(temp.path().join("models")).expect("store");

        let installed = store
            .install_manifest(&BytesDownloader { bytes }, &manifest)
            .await
            .expect("download model");

        assert!(Path::new(&installed.path).is_file());
        assert_eq!(
            installed.sha256.as_deref(),
            Some(manifest.sha256.as_deref().unwrap())
        );
    }

    #[tokio::test]
    async fn model_store_deletes_installed_model_tree() {
        let temp = TempDir::new().expect("temp dir");
        let source = temp.path().join("ggml-fixture.bin");
        std::fs::write(&source, b"fixture-model").expect("write source");
        let manifest = fixture_manifest(ModelSource::LocalFile {
            path: source.display().to_string(),
        });
        let store = ModelStore::new(temp.path().join("models")).expect("store");
        let installed = store
            .install_manifest(&NoopDownloader, &manifest)
            .await
            .expect("install local model");
        let installed_path = PathBuf::from(&installed.path);

        assert!(
            store
                .delete_model(&manifest.model_id)
                .expect("delete model")
        );

        assert!(!installed_path.exists());
        assert!(
            store
                .find_local_model(&manifest.model_id)
                .expect("find deleted")
                .is_none()
        );
    }

    #[test]
    fn model_store_legacy_discovery_accepts_only_ggml_bin_files() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("legacy-whisper");
        std::fs::create_dir_all(&model_dir).expect("create legacy dir");
        std::fs::write(model_dir.join("notes.txt"), b"not a model").expect("write junk");
        std::fs::write(model_dir.join("custom.bin"), b"not accepted").expect("write custom bin");
        std::fs::write(model_dir.join("ggml-base.bin"), b"accepted").expect("write model");
        let store = ModelStore::new(&root).expect("store");

        let model = store
            .find_local_model("legacy-whisper")
            .expect("find legacy model")
            .expect("legacy model");

        assert!(model.path.ends_with("ggml-base.bin"));
        assert_eq!(model.model_id, "legacy-whisper");
    }

    #[test]
    fn model_store_legacy_discovery_ignores_junk_directories() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("junk");
        std::fs::create_dir_all(&model_dir).expect("create junk dir");
        std::fs::write(model_dir.join("README.md"), b"not a model").expect("write junk");
        std::fs::write(model_dir.join("custom.bin"), b"not a legacy model")
            .expect("write custom bin");
        let store = ModelStore::new(&root).expect("store");

        assert!(
            store
                .find_local_model("junk")
                .expect("find junk dir")
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn model_store_legacy_discovery_ignores_model_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("models");
        let model_dir = root.join("linked");
        let target = temp.path().join("ggml-linked.bin");
        std::fs::create_dir_all(&model_dir).expect("create linked dir");
        std::fs::write(&target, b"target").expect("write target");
        symlink(&target, model_dir.join("ggml-linked.bin")).expect("create symlink");
        let store = ModelStore::new(&root).expect("store");

        assert!(
            store
                .find_local_model("linked")
                .expect("find linked dir")
                .is_none()
        );
    }

    fn fixture_manifest(source: ModelSource) -> ModelManifest {
        ModelManifest {
            model_id: "whisper-fixture".to_string(),
            display_name: "Whisper fixture".to_string(),
            family: ModelFamily::Whisper,
            format: ModelFormat::WhisperCppGgml,
            runtime_backend: Some(LocalModelRuntimeBackend::WhisperRs),
            version: Some("test".to_string()),
            size_bytes: None,
            sha256: None,
            checksum: None,
            license: Some("MIT".to_string()),
            sources: vec![source],
            capabilities: vec![ModelCapability::SpeechTranscription],
        }
    }

    struct NoopDownloader;

    impl ArtifactDownloader for NoopDownloader {
        async fn download_to_writer<W>(
            &self,
            _url: &str,
            _writer: &mut W,
            _max_bytes: Option<u64>,
        ) -> Result<(), ArtifactInstallError>
        where
            W: Write + ?Sized + Send,
        {
            Err(ArtifactInstallError::download("unexpected download"))
        }
    }

    struct BytesDownloader {
        bytes: Vec<u8>,
    }

    impl ArtifactDownloader for BytesDownloader {
        async fn download_to_writer<W>(
            &self,
            _url: &str,
            writer: &mut W,
            _max_bytes: Option<u64>,
        ) -> Result<(), ArtifactInstallError>
        where
            W: Write + ?Sized + Send,
        {
            writer
                .write_all(&self.bytes)
                .map_err(|error| ArtifactInstallError::download(error.to_string()))
        }
    }
}
