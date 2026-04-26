#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AudioInputBackend {
    WebMediaRecorder,
    CpalNative,
    SystemAudio,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceId {
    pub backend: AudioInputBackend,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioSampleFormat {
    F32,
    I16,
    U16,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFrameFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_format: AudioSampleFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputConfig {
    pub device: Option<AudioDeviceId>,
    pub format: AudioFrameFormat,
    pub echo_cancellation: Option<bool>,
    pub noise_suppression: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSessionId {
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureSessionStatus {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CaptureEvent {
    StatusChanged {
        session_id: CaptureSessionId,
        status: CaptureSessionStatus,
    },
    FrameFormatSelected {
        session_id: CaptureSessionId,
        format: AudioFrameFormat,
    },
    Completed {
        session_id: CaptureSessionId,
        duration_ms: u64,
    },
    Failed {
        session_id: CaptureSessionId,
        error: AudioInputError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputError {
    pub kind: AudioInputErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioInputErrorKind {
    PermissionDenied,
    DeviceUnavailable,
    UnsupportedConfig,
    BackendUnavailable,
    StreamInterrupted,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::{
        AudioFrameFormat, AudioInputBackend, AudioInputConfig, AudioSampleFormat, CaptureEvent,
        CaptureSessionId, CaptureSessionStatus,
    };

    #[test]
    fn input_config_serializes_with_stable_backend_shape() {
        let config = AudioInputConfig {
            device: None,
            format: AudioFrameFormat {
                sample_rate_hz: 48_000,
                channels: 1,
                sample_format: AudioSampleFormat::F32,
            },
            echo_cancellation: Some(true),
            noise_suppression: Some(true),
        };

        let value = serde_json::to_value(config).expect("serialize config");

        assert_eq!(value["format"]["sampleRateHz"], 48_000);
        assert_eq!(value["format"]["sampleFormat"], "f32");
        assert_eq!(value["echoCancellation"], true);
    }

    #[test]
    fn capture_event_serializes_with_tagged_kind() {
        let event = CaptureEvent::StatusChanged {
            session_id: CaptureSessionId {
                value: "capture-1".to_string(),
            },
            status: CaptureSessionStatus::Recording,
        };

        let value = serde_json::to_value(event).expect("serialize event");

        assert_eq!(value["kind"], "statusChanged");
        assert_eq!(value["sessionId"]["value"], "capture-1");
        assert_eq!(value["status"], "recording");
    }

    #[test]
    fn custom_backend_serializes_with_explicit_id() {
        let value = serde_json::to_value(AudioInputBackend::Custom {
            id: "vendor-native".to_string(),
        })
        .expect("serialize backend");

        assert_eq!(value["kind"], "custom");
        assert_eq!(value["id"], "vendor-native");
    }
}
