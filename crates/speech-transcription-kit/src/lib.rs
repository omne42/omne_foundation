#![forbid(unsafe_code)]

pub use audio_media_kit::AudioAssetRef;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TranscriptionAudioSource {
    LocalFile {
        path: String,
        #[serde(rename = "mimeType")]
        mime_type: Option<String>,
        sha256: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionProviderSelection {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionProviderRegistry {
    pub providers: Vec<TranscriptionProviderDescriptor>,
    pub default_provider_id: Option<String>,
}

impl TranscriptionProviderRegistry {
    pub fn find_provider(&self, provider_id: &str) -> Option<&TranscriptionProviderDescriptor> {
        self.providers
            .iter()
            .find(|provider| provider.provider_id == provider_id)
    }

    pub fn find_model(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Option<&TranscriptionModelDescriptor> {
        self.find_provider(provider_id)?
            .models
            .iter()
            .find(|descriptor| descriptor.model == model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionProviderDescriptor {
    pub provider_id: String,
    pub display_name: String,
    pub kind: TranscriptionProviderKind,
    pub default_model: Option<String>,
    pub models: Vec<TranscriptionModelDescriptor>,
    pub capabilities: Vec<TranscriptionProviderCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TranscriptionProviderKind {
    #[serde(rename = "openaiCompatible")]
    OpenAiCompatible,
    LocalWhisper,
    Custom {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionModelDescriptor {
    pub model: String,
    pub display_name: Option<String>,
    pub capabilities: Vec<TranscriptionProviderCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TranscriptionProviderCapability {
    LanguageSelection,
    Prompt,
    SegmentTimestamps,
    WordTimestamps,
    Translation,
    VoiceActivityDetection,
    StreamingResults,
    LocalModelExecution,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionOptions {
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub timestamps: TranscriptionTimestampMode,
}

impl Default for TranscriptionOptions {
    fn default() -> Self {
        Self {
            language: None,
            prompt: None,
            timestamps: TranscriptionTimestampMode::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionTimestampMode {
    None,
    Segment,
    Word,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionRequest {
    pub audio: TranscriptionAudioSource,
    pub provider: TranscriptionProviderSelection,
    pub options: TranscriptionOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionResult {
    pub text: String,
    pub segments: Vec<TranscriptionSegment>,
    pub provenance: TranscriptionProviderProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionProviderProvenance {
    pub provider_id: String,
    pub model: String,
    pub config_fingerprint: Option<String>,
    pub audio_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionJob {
    pub job_id: String,
    pub audio: AudioAssetRef,
    pub provider: TranscriptionProviderSelection,
    pub options: TranscriptionOptions,
    pub status: TranscriptionJobStatus,
    pub result: Option<TranscriptionResult>,
    pub error: Option<TranscriptionError>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionJobStatus {
    Pending,
    Preparing,
    Transcribing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionError {
    pub kind: TranscriptionErrorKind,
    pub message: String,
    pub provider_error_code: Option<String>,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionErrorKind {
    AuthenticationFailed,
    PermissionDenied,
    RateLimited,
    ProviderUnavailable,
    ProviderRejected,
    InvalidProviderResponse,
    ModelUnavailable,
    UnsupportedAudioFormat,
    AudioTooLarge,
    Timeout,
    Cancelled,
    InvalidRequest,
    Network,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::{
        AudioAssetRef, TranscriptionAudioSource, TranscriptionJob, TranscriptionJobStatus,
        TranscriptionModelDescriptor, TranscriptionOptions, TranscriptionProviderCapability,
        TranscriptionProviderDescriptor, TranscriptionProviderKind,
        TranscriptionProviderProvenance, TranscriptionProviderRegistry,
        TranscriptionProviderSelection, TranscriptionRequest, TranscriptionResult,
        TranscriptionTimestampMode,
    };

    #[test]
    fn default_options_do_not_request_timestamps() {
        assert_eq!(
            TranscriptionOptions::default().timestamps,
            TranscriptionTimestampMode::None
        );
    }

    #[test]
    fn request_serializes_with_camel_case_fields() {
        let request = TranscriptionRequest {
            audio: TranscriptionAudioSource::LocalFile {
                path: "/tmp/capture.webm".to_string(),
                mime_type: Some("audio/webm".to_string()),
                sha256: Some("sha256:abc".to_string()),
            },
            provider: TranscriptionProviderSelection {
                provider_id: "openai-compatible".to_string(),
                model: "whisper-1".to_string(),
            },
            options: TranscriptionOptions {
                language: Some("zh".to_string()),
                prompt: None,
                timestamps: TranscriptionTimestampMode::Segment,
            },
        };

        let value = serde_json::to_value(request).expect("serialize request");
        assert_eq!(value["audio"]["kind"], "localFile");
        assert_eq!(value["audio"]["mimeType"], "audio/webm");
        assert_eq!(value["provider"]["providerId"], "openai-compatible");
        assert_eq!(value["options"]["timestamps"], "segment");
    }

    #[test]
    fn job_status_uses_stable_wire_names() {
        assert_eq!(
            serde_json::to_string(&TranscriptionJobStatus::Transcribing).expect("serialize"),
            "\"transcribing\""
        );
    }

    #[test]
    fn transcription_job_serializes_stable_asset_and_result_shape() {
        let job = TranscriptionJob {
            job_id: "job-1".to_string(),
            audio: AudioAssetRef {
                asset_id: "asset-1".to_string(),
                path: "/tmp/capture.webm".to_string(),
                file_name: "capture.webm".to_string(),
                mime_type: "audio/webm".to_string(),
                duration_ms: Some(1200),
                sha256: Some("sha256:abc".to_string()),
            },
            provider: TranscriptionProviderSelection {
                provider_id: "openai-compatible".to_string(),
                model: "whisper-1".to_string(),
            },
            options: TranscriptionOptions::default(),
            status: TranscriptionJobStatus::Completed,
            result: Some(TranscriptionResult {
                text: "hello".to_string(),
                segments: Vec::new(),
                provenance: TranscriptionProviderProvenance {
                    provider_id: "openai-compatible".to_string(),
                    model: "whisper-1".to_string(),
                    config_fingerprint: None,
                    audio_sha256: Some("sha256:abc".to_string()),
                },
            }),
            error: None,
            created_at: "2026-04-26T00:00:00Z".to_string(),
            updated_at: "2026-04-26T00:00:01Z".to_string(),
        };

        let value = serde_json::to_value(job).expect("serialize job");

        assert_eq!(value["jobId"], "job-1");
        assert_eq!(value["audio"]["assetId"], "asset-1");
        assert_eq!(value["audio"]["mimeType"], "audio/webm");
        assert_eq!(value["result"]["provenance"]["audioSha256"], "sha256:abc");
    }

    #[test]
    fn provider_descriptor_serializes_capabilities_and_models() {
        let descriptor = openai_provider_descriptor();

        let value = serde_json::to_value(descriptor).expect("serialize descriptor");

        assert_eq!(value["providerId"], "openai-compatible");
        assert_eq!(value["kind"]["kind"], "openaiCompatible");
        assert_eq!(value["defaultModel"], "whisper-1");
        assert_eq!(value["models"][0]["model"], "whisper-1");
        assert_eq!(value["capabilities"][0]["kind"], "languageSelection");
    }

    #[test]
    fn provider_registry_serializes_default_and_supports_lookup() {
        let registry = TranscriptionProviderRegistry {
            providers: vec![openai_provider_descriptor()],
            default_provider_id: Some("openai-compatible".to_string()),
        };

        let value = serde_json::to_value(&registry).expect("serialize registry");

        assert_eq!(value["defaultProviderId"], "openai-compatible");
        assert_eq!(value["providers"][0]["providerId"], "openai-compatible");
        assert!(registry.find_provider("openai-compatible").is_some());
        assert!(
            registry
                .find_model("openai-compatible", "whisper-1")
                .is_some()
        );
        assert!(
            registry
                .find_model("openai-compatible", "missing")
                .is_none()
        );
    }

    #[test]
    fn failed_job_serializes_structured_transcription_error() {
        let job = TranscriptionJob {
            job_id: "job-1".to_string(),
            audio: AudioAssetRef {
                asset_id: "asset-1".to_string(),
                path: "/tmp/capture.webm".to_string(),
                file_name: "capture.webm".to_string(),
                mime_type: "audio/webm".to_string(),
                duration_ms: Some(1200),
                sha256: Some("sha256:abc".to_string()),
            },
            provider: TranscriptionProviderSelection {
                provider_id: "openai-compatible".to_string(),
                model: "whisper-1".to_string(),
            },
            options: TranscriptionOptions::default(),
            status: TranscriptionJobStatus::Failed,
            result: None,
            error: Some(super::TranscriptionError {
                kind: super::TranscriptionErrorKind::RateLimited,
                message: "provider rate limited the request".to_string(),
                provider_error_code: Some("rate_limit_exceeded".to_string()),
                retryable: true,
                retry_after_ms: Some(1000),
            }),
            created_at: "2026-04-26T00:00:00Z".to_string(),
            updated_at: "2026-04-26T00:00:01Z".to_string(),
        };

        let value = serde_json::to_value(job).expect("serialize failed job");

        assert_eq!(value["status"], "failed");
        assert_eq!(value["error"]["kind"], "rateLimited");
        assert_eq!(value["error"]["providerErrorCode"], "rate_limit_exceeded");
        assert_eq!(value["error"]["retryable"], true);
        assert_eq!(value["error"]["retryAfterMs"], 1000);
    }

    fn openai_provider_descriptor() -> TranscriptionProviderDescriptor {
        TranscriptionProviderDescriptor {
            provider_id: "openai-compatible".to_string(),
            display_name: "OpenAI-compatible".to_string(),
            kind: TranscriptionProviderKind::OpenAiCompatible,
            default_model: Some("whisper-1".to_string()),
            models: vec![TranscriptionModelDescriptor {
                model: "whisper-1".to_string(),
                display_name: Some("Whisper 1".to_string()),
                capabilities: vec![
                    TranscriptionProviderCapability::LanguageSelection,
                    TranscriptionProviderCapability::Prompt,
                    TranscriptionProviderCapability::SegmentTimestamps,
                ],
            }],
            capabilities: vec![
                TranscriptionProviderCapability::LanguageSelection,
                TranscriptionProviderCapability::Prompt,
                TranscriptionProviderCapability::SegmentTimestamps,
            ],
        }
    }
}
