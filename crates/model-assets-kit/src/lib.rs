#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelManifest {
    pub model_id: String,
    pub display_name: String,
    pub family: ModelFamily,
    pub format: ModelFormat,
    pub version: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub license: Option<String>,
    pub sources: Vec<ModelSource>,
    pub capabilities: Vec<ModelCapability>,
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
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelRef {
    pub model_id: String,
    pub path: String,
    pub format: ModelFormat,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInstallRequest {
    pub model_id: String,
    pub source: ModelSource,
    pub expected_sha256: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::{
        LocalModelRef, LocalModelRuntimeBackend, ModelCapability, ModelFamily, ModelFormat,
        ModelInstallProgress, ModelInstallStatus, ModelManifest, ModelSource,
    };

    #[test]
    fn model_manifest_serializes_stable_sources_and_capabilities() {
        let manifest = ModelManifest {
            model_id: "whisper-small-q5".to_string(),
            display_name: "Whisper Small Q5".to_string(),
            family: ModelFamily::Whisper,
            format: ModelFormat::Gguf,
            version: Some("v1".to_string()),
            size_bytes: Some(512),
            sha256: Some("sha256:abc".to_string()),
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
    fn local_model_ref_and_runtime_backend_are_separate() {
        let local = LocalModelRef {
            model_id: "whisper-small-q5".to_string(),
            path: "/models/model.gguf".to_string(),
            format: ModelFormat::Gguf,
            sha256: Some("sha256:abc".to_string()),
        };
        let backend = LocalModelRuntimeBackend::WhisperCppSidecar;

        let local_value = serde_json::to_value(local).expect("serialize local model");
        let backend_value = serde_json::to_value(backend).expect("serialize backend");

        assert_eq!(local_value["path"], "/models/model.gguf");
        assert_eq!(backend_value["kind"], "whisperCppSidecar");
    }
}
