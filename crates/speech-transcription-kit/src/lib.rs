#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptionAudioSource {
    InlineBytes {
        data: Vec<u8>,
        file_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranscriptionResponseFormat {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "srt")]
    Srt,
    #[serde(rename = "verbose_json")]
    VerboseJson,
    #[serde(rename = "vtt")]
    Vtt,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<TranscriptionResponseFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionRequest {
    pub audio: TranscriptionAudioSource,
    #[serde(default)]
    pub options: TranscriptionOptions,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,
    pub provenance: TranscriptionProviderProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionProviderProvenance {
    pub provider_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionError {
    pub kind: TranscriptionErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_code: Option<String>,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    use super::*;

    #[test]
    fn request_carries_inline_audio_and_options() {
        let request = TranscriptionRequest {
            audio: TranscriptionAudioSource::InlineBytes {
                data: vec![1, 2, 3],
                file_name: "sample.wav".to_string(),
                media_type: Some("audio/wav".to_string()),
            },
            options: TranscriptionOptions {
                model: Some("whisper-1".to_string()),
                language: Some("en".to_string()),
                prompt: Some("technical vocabulary".to_string()),
                response_format: Some(TranscriptionResponseFormat::VerboseJson),
                temperature: Some(0.2),
            },
        };

        let encoded = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(encoded["audio"]["kind"], "inline_bytes");
        assert_eq!(encoded["options"]["response_format"], "verbose_json");
    }

    #[test]
    fn result_carries_segments_and_provider_provenance() {
        let result = TranscriptionResult {
            text: "hello".to_string(),
            segments: vec![TranscriptionSegment {
                start_ms: 0,
                end_ms: 1200,
                text: "hello".to_string(),
            }],
            provenance: TranscriptionProviderProvenance {
                provider_id: "openai-compatible".to_string(),
                model: "whisper-1".to_string(),
                config_fingerprint: None,
                audio_sha256: Some("sha256:abc".to_string()),
            },
        };

        let encoded = serde_json::to_value(&result).expect("serialize result");

        assert_eq!(encoded["segments"][0]["start_ms"], 0);
        assert_eq!(encoded["provenance"]["provider_id"], "openai-compatible");
        assert_eq!(encoded["provenance"]["audio_sha256"], "sha256:abc");
    }

    #[test]
    fn error_kind_uses_stable_wire_names() {
        let error = TranscriptionError {
            kind: TranscriptionErrorKind::RateLimited,
            message: "provider rate limited the request".to_string(),
            provider_error_code: Some("rate_limit_exceeded".to_string()),
            retryable: true,
            retry_after_ms: Some(1000),
        };

        let encoded = serde_json::to_value(&error).expect("serialize error");

        assert_eq!(encoded["kind"], "rate_limited");
        assert_eq!(encoded["provider_error_code"], "rate_limit_exceeded");
        assert_eq!(encoded["retryable"], true);
        assert_eq!(encoded["retry_after_ms"], 1000);
    }
}
