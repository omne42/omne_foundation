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
}
