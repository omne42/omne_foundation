#![forbid(unsafe_code)]

use std::path::Path;

use serde::{Deserialize, Serialize};

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
    use std::path::Path;

    use super::{
        LocalModelRuntimeBackend, ModelCapability, ModelChecksumAlgorithm, ModelFamily,
        ModelFormat, ModelInstallProgress, ModelInstallStatus, ModelManifest, ModelSource,
        WHISPER_CPP_MODEL_SPECS, infer_whisper_cpp_model_id, local_model_ref_from_whisper_path,
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
}
