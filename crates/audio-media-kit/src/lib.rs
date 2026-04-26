#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioAssetRef {
    pub asset_id: String,
    pub path: String,
    pub file_name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub duration_ms: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioAsset {
    pub reference: AudioAssetRef,
    pub size_bytes: Option<u64>,
    pub format: Option<AudioMediaFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AudioContainerFormat {
    Wav,
    #[serde(rename = "webm")]
    WebM,
    Mp4,
    Ogg,
    Mp3,
    Flac,
    RawPcm,
    Custom {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AudioCodec {
    PcmS16Le,
    PcmF32Le,
    Opus,
    Aac,
    Mp3,
    Flac,
    Vorbis,
    Unknown,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMediaFormat {
    pub container: Option<AudioContainerFormat>,
    pub codec: Option<AudioCodec>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u16>,
    pub bit_rate_bps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPreprocessTarget {
    pub mime_type: String,
    pub container: AudioContainerFormat,
    pub codec: AudioCodec,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

impl AudioPreprocessTarget {
    pub fn whisper_pcm_wav() -> Self {
        Self {
            mime_type: "audio/wav".to_string(),
            container: AudioContainerFormat::Wav,
            codec: AudioCodec::PcmS16Le,
            sample_rate_hz: 16_000,
            channels: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProcessingBudget {
    pub max_duration_ms: Option<u64>,
    pub max_input_bytes: Option<u64>,
    pub max_output_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AudioProcessingStep {
    Decode,
    Resample { sample_rate_hz: u32 },
    MixDown { channels: u16 },
    NormalizePeak { target_peak_milli_db: i32 },
    Encode { target: AudioPreprocessTarget },
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPreprocessRequest {
    pub input: AudioAssetRef,
    pub target: AudioPreprocessTarget,
    pub budget: AudioProcessingBudget,
    pub steps: Vec<AudioProcessingStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPreprocessResult {
    pub source: AudioAssetRef,
    pub output: AudioAssetRef,
    pub target: AudioPreprocessTarget,
    pub provenance: AudioPreprocessProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPreprocessProvenance {
    pub pipeline_id: Option<String>,
    pub steps: Vec<AudioProcessingStep>,
    pub source_sha256: Option<String>,
    pub output_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMediaError {
    pub kind: AudioMediaErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioMediaErrorKind {
    UnsupportedFormat,
    DecodeFailed,
    ResampleFailed,
    EncodeFailed,
    BudgetExceeded,
    Io,
    Internal,
}

impl AudioMediaErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "audio_media.unsupported_format",
            Self::DecodeFailed => "audio_media.decode_failed",
            Self::ResampleFailed => "audio_media.resample_failed",
            Self::EncodeFailed => "audio_media.encode_failed",
            Self::BudgetExceeded => "audio_media.budget_exceeded",
            Self::Io => "audio_media.io",
            Self::Internal => "audio_media.internal",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(self, Self::Io | Self::Internal)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioAsset, AudioAssetRef, AudioCodec, AudioContainerFormat, AudioMediaErrorKind,
        AudioMediaFormat, AudioPreprocessRequest, AudioPreprocessTarget, AudioProcessingBudget,
        AudioProcessingStep,
    };

    fn sample_asset() -> AudioAssetRef {
        AudioAssetRef {
            asset_id: "asset-1".to_string(),
            path: "/tmp/capture.webm".to_string(),
            file_name: "capture.webm".to_string(),
            mime_type: "audio/webm".to_string(),
            duration_ms: Some(1200),
            sha256: Some("sha256:abc".to_string()),
        }
    }

    #[test]
    fn audio_asset_serializes_stable_metadata_shape() {
        let asset = AudioAsset {
            reference: sample_asset(),
            size_bytes: Some(4096),
            format: Some(AudioMediaFormat {
                container: Some(AudioContainerFormat::WebM),
                codec: Some(AudioCodec::Opus),
                sample_rate_hz: Some(48_000),
                channels: Some(1),
                bit_rate_bps: None,
            }),
        };

        let value = serde_json::to_value(asset).expect("serialize asset");

        assert_eq!(value["reference"]["assetId"], "asset-1");
        assert_eq!(value["reference"]["mimeType"], "audio/webm");
        assert_eq!(value["format"]["container"]["kind"], "webm");
        assert_eq!(value["format"]["codec"]["kind"], "opus");
        assert_eq!(value["format"]["sampleRateHz"], 48_000);
    }

    #[test]
    fn whisper_target_is_stable_pcm_wav_contract() {
        let target = AudioPreprocessTarget::whisper_pcm_wav();
        let value = serde_json::to_value(target).expect("serialize target");

        assert_eq!(value["mimeType"], "audio/wav");
        assert_eq!(value["container"]["kind"], "wav");
        assert_eq!(value["codec"]["kind"], "pcmS16Le");
        assert_eq!(value["sampleRateHz"], 16_000);
        assert_eq!(value["channels"], 1);
    }

    #[test]
    fn preprocess_request_serializes_pipeline_and_budget() {
        let target = AudioPreprocessTarget::whisper_pcm_wav();
        let request = AudioPreprocessRequest {
            input: sample_asset(),
            target: target.clone(),
            budget: AudioProcessingBudget {
                max_duration_ms: Some(30_000),
                max_input_bytes: Some(25 * 1024 * 1024),
                max_output_bytes: Some(10 * 1024 * 1024),
            },
            steps: vec![
                AudioProcessingStep::Decode,
                AudioProcessingStep::Resample {
                    sample_rate_hz: target.sample_rate_hz,
                },
                AudioProcessingStep::MixDown {
                    channels: target.channels,
                },
                AudioProcessingStep::Encode { target },
            ],
        };

        let value = serde_json::to_value(request).expect("serialize request");

        assert_eq!(value["target"]["mimeType"], "audio/wav");
        assert_eq!(value["budget"]["maxDurationMs"], 30_000);
        assert_eq!(value["steps"][0]["kind"], "decode");
        assert_eq!(value["steps"][1]["sampleRateHz"], 16_000);
        assert_eq!(value["steps"][3]["target"]["codec"]["kind"], "pcmS16Le");
    }

    #[test]
    fn preprocess_request_json_round_trips_and_ignores_unknown_fields() {
        let raw = serde_json::json!({
            "input": {
                "assetId": "asset-1",
                "path": "/tmp/capture.webm",
                "fileName": "capture.webm",
                "mimeType": "audio/webm",
                "durationMs": 1200,
                "sha256": null,
                "futureField": true
            },
            "target": {
                "mimeType": "audio/wav",
                "container": {"kind": "wav", "futureField": true},
                "codec": {"kind": "pcmS16Le", "futureField": true},
                "sampleRateHz": 16000,
                "channels": 1,
                "futureField": true
            },
            "budget": {
                "maxDurationMs": 30000,
                "maxInputBytes": null,
                "maxOutputBytes": null,
                "futureField": true
            },
            "steps": [
                {"kind": "decode", "futureField": true},
                {"kind": "resample", "sampleRateHz": 16000, "futureField": true}
            ],
            "futureField": "ignored"
        });

        let request: AudioPreprocessRequest =
            serde_json::from_value(raw).expect("deserialize preprocess request");

        assert_eq!(request.input.asset_id, "asset-1");
        assert_eq!(request.target.sample_rate_hz, 16_000);
        assert_eq!(request.steps.len(), 2);
    }

    #[test]
    fn audio_media_error_kind_exposes_stable_code_and_retry_hint() {
        assert_eq!(
            AudioMediaErrorKind::UnsupportedFormat.code(),
            "audio_media.unsupported_format"
        );
        assert!(!AudioMediaErrorKind::UnsupportedFormat.retryable());
        assert!(AudioMediaErrorKind::Io.retryable());
    }
}
