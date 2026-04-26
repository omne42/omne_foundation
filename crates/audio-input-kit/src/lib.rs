#![forbid(unsafe_code)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputDevice {
    pub id: AudioDeviceId,
    pub label: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioSampleFormat {
    F32,
    I16,
    U16,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

impl AudioInputErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::PermissionDenied => "audio_input.permission_denied",
            Self::DeviceUnavailable => "audio_input.device_unavailable",
            Self::UnsupportedConfig => "audio_input.unsupported_config",
            Self::BackendUnavailable => "audio_input.backend_unavailable",
            Self::StreamInterrupted => "audio_input.stream_interrupted",
            Self::Internal => "audio_input.internal",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::DeviceUnavailable
                | Self::BackendUnavailable
                | Self::StreamInterrupted
                | Self::Internal
        )
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct AudioInputRuntimeError {
    pub kind: AudioInputErrorKind,
    pub message: String,
}

impl AudioInputRuntimeError {
    pub fn new(kind: AudioInputErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn as_error(&self) -> AudioInputError {
        AudioInputError {
            kind: self.kind,
            message: self.message.clone(),
        }
    }
}

pub struct CpalInputStream {
    stream: cpal::Stream,
    format: AudioFrameFormat,
    errors: Arc<Mutex<Vec<AudioInputError>>>,
}

impl CpalInputStream {
    pub fn format(&self) -> AudioFrameFormat {
        self.format
    }

    pub fn drain_errors(&self) -> Vec<AudioInputError> {
        let mut errors = self
            .errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        errors.drain(..).collect()
    }

    pub fn pause(&self) -> Result<(), AudioInputRuntimeError> {
        self.stream.pause().map_err(audio_error_from_pause)
    }
}

pub fn list_cpal_input_devices() -> Result<Vec<AudioInputDevice>, AudioInputRuntimeError> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .map_err(|error| audio_error(AudioInputErrorKind::BackendUnavailable, error))?;

    devices
        .map(|device| {
            let label = device
                .name()
                .map_err(|error| audio_error(AudioInputErrorKind::DeviceUnavailable, error))?;
            Ok(AudioInputDevice {
                id: AudioDeviceId {
                    backend: AudioInputBackend::CpalNative,
                    id: label.clone(),
                },
                is_default: default_name.as_deref() == Some(label.as_str()),
                label,
            })
        })
        .collect()
}

pub fn start_cpal_mono_input_stream<F>(
    config: &AudioInputConfig,
    on_frame: F,
) -> Result<CpalInputStream, AudioInputRuntimeError>
where
    F: FnMut(&[f32], AudioFrameFormat) + Send + 'static,
{
    let host = cpal::default_host();
    let device = resolve_cpal_input_device(&host, config.device.as_ref())?;
    let supported = select_cpal_input_config(&device, config.format)?;
    let selected_sample_format = supported.sample_format();
    let stream_config = supported.config();
    let format = AudioFrameFormat {
        sample_rate_hz: stream_config.sample_rate.0,
        channels: stream_config.channels,
        sample_format: audio_sample_format_from_cpal(selected_sample_format),
    };
    let errors = Arc::new(Mutex::new(Vec::new()));

    let stream = match selected_sample_format {
        cpal::SampleFormat::F32 => build_cpal_stream(
            &device,
            &stream_config,
            format,
            on_frame,
            Arc::clone(&errors),
            mono_from_f32,
        )?,
        cpal::SampleFormat::I16 => build_cpal_stream(
            &device,
            &stream_config,
            format,
            on_frame,
            Arc::clone(&errors),
            mono_from_i16,
        )?,
        cpal::SampleFormat::U16 => build_cpal_stream(
            &device,
            &stream_config,
            format,
            on_frame,
            Arc::clone(&errors),
            mono_from_u16,
        )?,
        sample_format => {
            return Err(AudioInputRuntimeError::new(
                AudioInputErrorKind::UnsupportedConfig,
                format!("unsupported CPAL input sample format: {sample_format:?}"),
            ));
        }
    };

    stream.play().map_err(audio_error_from_play)?;
    Ok(CpalInputStream {
        stream,
        format,
        errors,
    })
}

fn resolve_cpal_input_device(
    host: &cpal::Host,
    requested: Option<&AudioDeviceId>,
) -> Result<cpal::Device, AudioInputRuntimeError> {
    let Some(requested) = requested else {
        return host.default_input_device().ok_or_else(|| {
            AudioInputRuntimeError::new(
                AudioInputErrorKind::DeviceUnavailable,
                "no default CPAL input device is available",
            )
        });
    };

    if requested.backend != AudioInputBackend::CpalNative {
        return Err(AudioInputRuntimeError::new(
            AudioInputErrorKind::UnsupportedConfig,
            "requested audio input device is not a CPAL native device",
        ));
    }

    let devices = host
        .input_devices()
        .map_err(|error| audio_error(AudioInputErrorKind::BackendUnavailable, error))?;
    for device in devices {
        let name = device
            .name()
            .map_err(|error| audio_error(AudioInputErrorKind::DeviceUnavailable, error))?;
        if name == requested.id {
            return Ok(device);
        }
    }

    Err(AudioInputRuntimeError::new(
        AudioInputErrorKind::DeviceUnavailable,
        format!("CPAL input device not found: {}", requested.id),
    ))
}

fn select_cpal_input_config(
    device: &cpal::Device,
    requested: AudioFrameFormat,
) -> Result<cpal::SupportedStreamConfig, AudioInputRuntimeError> {
    select_cpal_input_config_from_ranges(
        device
            .supported_input_configs()
            .map_err(|error| audio_error(AudioInputErrorKind::UnsupportedConfig, error))?,
        requested,
    )
}

fn select_cpal_input_config_from_ranges(
    ranges: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
    requested: AudioFrameFormat,
) -> Result<cpal::SupportedStreamConfig, AudioInputRuntimeError> {
    if requested.sample_rate_hz == 0 || requested.channels == 0 {
        return Err(AudioInputRuntimeError::new(
            AudioInputErrorKind::UnsupportedConfig,
            "requested CPAL input format must have a positive sample rate and channel count",
        ));
    }

    let requested_sample_format = cpal_sample_format_from_audio(requested.sample_format);
    for range in ranges {
        if range.channels() != requested.channels {
            continue;
        }
        if requested_sample_format.is_some_and(|format| range.sample_format() != format) {
            continue;
        }
        if let Some(config) = range.try_with_sample_rate(cpal::SampleRate(requested.sample_rate_hz))
        {
            return Ok(config);
        }
    }

    Err(AudioInputRuntimeError::new(
        AudioInputErrorKind::UnsupportedConfig,
        format!(
            "requested CPAL input format is unsupported: {} Hz, {} channels, {:?}",
            requested.sample_rate_hz, requested.channels, requested.sample_format
        ),
    ))
}

fn build_cpal_stream<T, F, C>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
    format: AudioFrameFormat,
    mut on_frame: F,
    errors: Arc<Mutex<Vec<AudioInputError>>>,
    convert: C,
) -> Result<cpal::Stream, AudioInputRuntimeError>
where
    T: cpal::SizedSample,
    F: FnMut(&[f32], AudioFrameFormat) + Send + 'static,
    C: Fn(&[T], usize) -> Vec<f32> + Send + 'static,
{
    let channels = usize::from(stream_config.channels.max(1));
    device
        .build_input_stream(
            stream_config,
            move |data: &[T], _| {
                let mono = convert(data, channels);
                if !mono.is_empty() {
                    on_frame(&mono, format);
                }
            },
            move |error| {
                let mut errors = errors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                errors.push(cpal_stream_error_to_audio_error(error));
            },
            Some(Duration::from_millis(100)),
        )
        .map_err(audio_error_from_build)
}

fn mono_from_f32(data: &[f32], channels: usize) -> Vec<f32> {
    mono_from_interleaved(data, channels, |sample| *sample)
}

fn mono_from_i16(data: &[i16], channels: usize) -> Vec<f32> {
    mono_from_interleaved(data, channels, |sample| {
        (*sample as f32 / i16::MAX as f32).clamp(-1.0, 1.0)
    })
}

fn mono_from_u16(data: &[u16], channels: usize) -> Vec<f32> {
    mono_from_interleaved(data, channels, |sample| {
        ((*sample as f32 - 32768.0) / 32768.0).clamp(-1.0, 1.0)
    })
}

fn mono_from_interleaved<T>(
    data: &[T],
    channels: usize,
    mut sample_to_f32: impl FnMut(&T) -> f32,
) -> Vec<f32> {
    if channels == 0 {
        return Vec::new();
    }

    data.chunks(channels)
        .map(|frame| frame.iter().map(&mut sample_to_f32).sum::<f32>() / frame.len() as f32)
        .collect()
}

fn audio_sample_format_from_cpal(value: cpal::SampleFormat) -> AudioSampleFormat {
    match value {
        cpal::SampleFormat::F32 => AudioSampleFormat::F32,
        cpal::SampleFormat::I16 => AudioSampleFormat::I16,
        cpal::SampleFormat::U16 => AudioSampleFormat::U16,
        _ => AudioSampleFormat::Unknown,
    }
}

fn cpal_sample_format_from_audio(value: AudioSampleFormat) -> Option<cpal::SampleFormat> {
    match value {
        AudioSampleFormat::F32 => Some(cpal::SampleFormat::F32),
        AudioSampleFormat::I16 => Some(cpal::SampleFormat::I16),
        AudioSampleFormat::U16 => Some(cpal::SampleFormat::U16),
        AudioSampleFormat::Unknown => None,
    }
}

fn cpal_stream_error_to_audio_error(error: cpal::StreamError) -> AudioInputError {
    AudioInputError {
        kind: AudioInputErrorKind::StreamInterrupted,
        message: error.to_string(),
    }
}

fn audio_error(kind: AudioInputErrorKind, error: impl std::fmt::Display) -> AudioInputRuntimeError {
    AudioInputRuntimeError::new(kind, error.to_string())
}

fn audio_error_from_build(error: cpal::BuildStreamError) -> AudioInputRuntimeError {
    match error {
        cpal::BuildStreamError::DeviceNotAvailable => AudioInputRuntimeError::new(
            AudioInputErrorKind::DeviceUnavailable,
            "CPAL input device is not available",
        ),
        cpal::BuildStreamError::StreamConfigNotSupported => AudioInputRuntimeError::new(
            AudioInputErrorKind::UnsupportedConfig,
            "CPAL input stream config is not supported",
        ),
        cpal::BuildStreamError::InvalidArgument => AudioInputRuntimeError::new(
            AudioInputErrorKind::UnsupportedConfig,
            "CPAL input stream received an invalid argument",
        ),
        cpal::BuildStreamError::StreamIdOverflow => AudioInputRuntimeError::new(
            AudioInputErrorKind::Internal,
            "CPAL input stream id overflow",
        ),
        cpal::BuildStreamError::BackendSpecific { err } => audio_error(
            AudioInputErrorKind::BackendUnavailable,
            format!("CPAL backend error: {err}"),
        ),
    }
}

fn audio_error_from_play(error: cpal::PlayStreamError) -> AudioInputRuntimeError {
    match error {
        cpal::PlayStreamError::DeviceNotAvailable => AudioInputRuntimeError::new(
            AudioInputErrorKind::DeviceUnavailable,
            "CPAL input device is not available",
        ),
        cpal::PlayStreamError::BackendSpecific { err } => audio_error(
            AudioInputErrorKind::BackendUnavailable,
            format!("CPAL backend error: {err}"),
        ),
    }
}

fn audio_error_from_pause(error: cpal::PauseStreamError) -> AudioInputRuntimeError {
    match error {
        cpal::PauseStreamError::DeviceNotAvailable => AudioInputRuntimeError::new(
            AudioInputErrorKind::DeviceUnavailable,
            "CPAL input device is not available",
        ),
        cpal::PauseStreamError::BackendSpecific { err } => audio_error(
            AudioInputErrorKind::BackendUnavailable,
            format!("CPAL backend error: {err}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioFrameFormat, AudioInputBackend, AudioInputConfig, AudioInputErrorKind,
        AudioSampleFormat, CaptureEvent, CaptureSessionId, CaptureSessionStatus,
        cpal_stream_error_to_audio_error, mono_from_i16, mono_from_u16,
        select_cpal_input_config_from_ranges,
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

    #[test]
    fn input_config_json_round_trips_and_ignores_unknown_fields() {
        let raw = serde_json::json!({
            "device": null,
            "format": {
                "sampleRateHz": 16000,
                "channels": 1,
                "sampleFormat": "f32",
                "futureField": true
            },
            "echoCancellation": false,
            "noiseSuppression": true,
            "futureField": "ignored"
        });

        let config: AudioInputConfig =
            serde_json::from_value(raw).expect("deserialize input config");

        assert_eq!(config.format.sample_rate_hz, 16_000);
        assert_eq!(config.format.channels, 1);
        assert_eq!(config.format.sample_format, AudioSampleFormat::F32);
        assert_eq!(config.noise_suppression, Some(true));
    }

    #[test]
    fn interleaved_integer_frames_convert_to_mono_f32() {
        assert_eq!(
            mono_from_i16(&[i16::MAX, 0, 0, i16::MAX], 2),
            vec![0.5, 0.5]
        );
        assert_eq!(mono_from_u16(&[u16::MAX, 32768], 2).len(), 1);
    }

    #[test]
    fn requested_cpal_format_selects_matching_supported_config() {
        let ranges = vec![
            cpal::SupportedStreamConfigRange::new(
                2,
                cpal::SampleRate(44_100),
                cpal::SampleRate(48_000),
                cpal::SupportedBufferSize::Unknown,
                cpal::SampleFormat::F32,
            ),
            cpal::SupportedStreamConfigRange::new(
                1,
                cpal::SampleRate(16_000),
                cpal::SampleRate(16_000),
                cpal::SupportedBufferSize::Unknown,
                cpal::SampleFormat::I16,
            ),
        ];

        let selected = select_cpal_input_config_from_ranges(
            ranges,
            AudioFrameFormat {
                sample_rate_hz: 16_000,
                channels: 1,
                sample_format: AudioSampleFormat::I16,
            },
        )
        .expect("matching config");

        assert_eq!(selected.channels(), 1);
        assert_eq!(selected.sample_rate().0, 16_000);
        assert_eq!(selected.sample_format(), cpal::SampleFormat::I16);
    }

    #[test]
    fn requested_cpal_format_rejects_unsupported_config() {
        let ranges = vec![cpal::SupportedStreamConfigRange::new(
            2,
            cpal::SampleRate(44_100),
            cpal::SampleRate(48_000),
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )];

        let error = select_cpal_input_config_from_ranges(
            ranges,
            AudioFrameFormat {
                sample_rate_hz: 16_000,
                channels: 1,
                sample_format: AudioSampleFormat::I16,
            },
        )
        .expect_err("unsupported config");

        assert_eq!(error.kind, AudioInputErrorKind::UnsupportedConfig);
    }

    #[test]
    fn unknown_sample_format_is_wildcard_for_cpal_selection() {
        let ranges = vec![cpal::SupportedStreamConfigRange::new(
            1,
            cpal::SampleRate(16_000),
            cpal::SampleRate(16_000),
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )];

        let selected = select_cpal_input_config_from_ranges(
            ranges,
            AudioFrameFormat {
                sample_rate_hz: 16_000,
                channels: 1,
                sample_format: AudioSampleFormat::Unknown,
            },
        )
        .expect("wildcard sample format");

        assert_eq!(selected.sample_format(), cpal::SampleFormat::F32);
    }

    #[test]
    fn cpal_stream_errors_map_to_interrupted_errors() {
        let error = cpal_stream_error_to_audio_error(cpal::StreamError::DeviceNotAvailable);

        assert_eq!(error.kind, AudioInputErrorKind::StreamInterrupted);
        assert!(!error.message.is_empty());
    }

    #[test]
    fn audio_error_kind_exposes_stable_code_and_retry_hint() {
        assert_eq!(
            AudioInputErrorKind::StreamInterrupted.code(),
            "audio_input.stream_interrupted"
        );
        assert!(AudioInputErrorKind::StreamInterrupted.retryable());
        assert!(!AudioInputErrorKind::PermissionDenied.retryable());
    }
}
