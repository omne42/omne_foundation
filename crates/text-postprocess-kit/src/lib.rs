#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TextPostprocessSource {
    Transcript {
        text: String,
        source_id: Option<String>,
        language: Option<String>,
    },
    PlainText {
        text: String,
        source_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessProviderSelection {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessRequest {
    pub source: TextPostprocessSource,
    pub provider: TextPostprocessProviderSelection,
    pub options: TextPostprocessOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessOptions {
    pub mode: TextPostprocessMode,
    pub custom_instruction: Option<String>,
    pub temperature: Option<f32>,
    pub timeout_ms: Option<u64>,
}

impl Default for TextPostprocessOptions {
    fn default() -> Self {
        Self {
            mode: TextPostprocessMode::CleanTranscript,
            custom_instruction: None,
            temperature: None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextPostprocessMode {
    CleanTranscript,
    Concise,
    Formal,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessResult {
    pub text: String,
    pub provenance: TextPostprocessProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessProvenance {
    pub provider_id: String,
    pub model: String,
    pub source_id: Option<String>,
    pub config_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessJob {
    pub job_id: String,
    pub source_id: Option<String>,
    pub provider: TextPostprocessProviderSelection,
    pub options: TextPostprocessOptionsSnapshot,
    pub status: TextPostprocessJobStatus,
    pub result: Option<TextPostprocessResult>,
    pub error: Option<TextPostprocessError>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessOptionsSnapshot {
    pub mode: TextPostprocessMode,
    pub custom_instruction: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextPostprocessJobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPostprocessError {
    pub kind: TextPostprocessErrorKind,
    pub message: String,
    pub provider_error_code: Option<String>,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextPostprocessErrorKind {
    AuthenticationFailed,
    PermissionDenied,
    RateLimited,
    ProviderUnavailable,
    ProviderRejected,
    InvalidProviderResponse,
    ModelUnavailable,
    Timeout,
    Cancelled,
    InvalidRequest,
    Network,
    Internal,
}

impl TextPostprocessErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "text_postprocess.authentication_failed",
            Self::PermissionDenied => "text_postprocess.permission_denied",
            Self::RateLimited => "text_postprocess.rate_limited",
            Self::ProviderUnavailable => "text_postprocess.provider_unavailable",
            Self::ProviderRejected => "text_postprocess.provider_rejected",
            Self::InvalidProviderResponse => "text_postprocess.invalid_provider_response",
            Self::ModelUnavailable => "text_postprocess.model_unavailable",
            Self::Timeout => "text_postprocess.timeout",
            Self::Cancelled => "text_postprocess.cancelled",
            Self::InvalidRequest => "text_postprocess.invalid_request",
            Self::Network => "text_postprocess.network",
            Self::Internal => "text_postprocess.internal",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::ProviderUnavailable
                | Self::InvalidProviderResponse
                | Self::Timeout
                | Self::Network
                | Self::Internal
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TextPostprocessErrorKind, TextPostprocessMode, TextPostprocessOptions,
        TextPostprocessProviderSelection, TextPostprocessRequest, TextPostprocessSource,
    };

    #[test]
    fn request_serializes_stable_transcript_shape() {
        let request = TextPostprocessRequest {
            source: TextPostprocessSource::Transcript {
                text: "hello world".to_string(),
                source_id: Some("record-1".to_string()),
                language: Some("en".to_string()),
            },
            provider: TextPostprocessProviderSelection {
                provider_id: "ditto-openai-compatible".to_string(),
                model: "gpt-test".to_string(),
            },
            options: TextPostprocessOptions {
                mode: TextPostprocessMode::CleanTranscript,
                custom_instruction: None,
                temperature: Some(0.2),
                timeout_ms: Some(60_000),
            },
        };

        let value = serde_json::to_value(request).expect("serialize postprocess request");

        assert_eq!(value["source"]["kind"], "transcript");
        assert_eq!(value["source"]["sourceId"], "record-1");
        assert_eq!(value["provider"]["providerId"], "ditto-openai-compatible");
        assert_eq!(value["options"]["mode"], "cleanTranscript");
    }

    #[test]
    fn default_options_use_clean_transcript_mode() {
        assert_eq!(
            TextPostprocessOptions::default().mode,
            TextPostprocessMode::CleanTranscript
        );
    }

    #[test]
    fn request_json_round_trips_and_ignores_unknown_fields() {
        let raw = serde_json::json!({
            "source": {
                "kind": "plainText",
                "text": "hello",
                "sourceId": "note-1",
                "futureField": true
            },
            "provider": {
                "providerId": "ditto",
                "model": "gpt-test",
                "futureField": "ignored"
            },
            "options": {
                "mode": "formal",
                "customInstruction": null,
                "temperature": 0.2,
                "timeoutMs": 1000,
                "futureField": "ignored"
            },
            "futureField": "ignored"
        });

        let request: TextPostprocessRequest =
            serde_json::from_value(raw).expect("deserialize postprocess request");

        assert_eq!(request.provider.provider_id, "ditto");
        assert_eq!(request.options.mode, TextPostprocessMode::Formal);
    }

    #[test]
    fn text_postprocess_error_kind_exposes_stable_code_and_retry_hint() {
        assert_eq!(
            TextPostprocessErrorKind::RateLimited.code(),
            "text_postprocess.rate_limited"
        );
        assert!(TextPostprocessErrorKind::RateLimited.retryable());
        assert!(!TextPostprocessErrorKind::PermissionDenied.retryable());
    }
}
