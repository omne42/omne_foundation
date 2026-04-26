#![forbid(unsafe_code)]

use std::time::Duration;

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
}

impl CpalInputStream {
    pub fn format(&self) -> AudioFrameFormat {
        self.format
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
    let supported = device
        .default_input_config()
        .map_err(audio_error_from_default_config)?;
    let selected_sample_format = supported.sample_format();
    let stream_config: cpal::StreamConfig = supported.into();
    let format = AudioFrameFormat {
        sample_rate_hz: stream_config.sample_rate.0,
        channels: stream_config.channels,
        sample_format: audio_sample_format_from_cpal(selected_sample_format),
    };

    let stream = match selected_sample_format {
        cpal::SampleFormat::F32 => {
            build_cpal_stream(&device, &stream_config, format, on_frame, mono_from_f32)?
        }
        cpal::SampleFormat::I16 => {
            build_cpal_stream(&device, &stream_config, format, on_frame, mono_from_i16)?
        }
        cpal::SampleFormat::U16 => {
            build_cpal_stream(&device, &stream_config, format, on_frame, mono_from_u16)?
        }
        sample_format => {
            return Err(AudioInputRuntimeError::new(
                AudioInputErrorKind::UnsupportedConfig,
                format!("unsupported CPAL input sample format: {sample_format:?}"),
            ));
        }
    };

    stream.play().map_err(audio_error_from_play)?;
    Ok(CpalInputStream { stream, format })
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

fn build_cpal_stream<T, F, C>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
    format: AudioFrameFormat,
    mut on_frame: F,
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
            |_error| {},
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

fn audio_error(kind: AudioInputErrorKind, error: impl std::fmt::Display) -> AudioInputRuntimeError {
    AudioInputRuntimeError::new(kind, error.to_string())
}

fn audio_error_from_default_config(
    error: cpal::DefaultStreamConfigError,
) -> AudioInputRuntimeError {
    match error {
        cpal::DefaultStreamConfigError::DeviceNotAvailable => AudioInputRuntimeError::new(
            AudioInputErrorKind::DeviceUnavailable,
            "CPAL input device is not available",
        ),
        cpal::DefaultStreamConfigError::StreamTypeNotSupported => AudioInputRuntimeError::new(
            AudioInputErrorKind::UnsupportedConfig,
            "CPAL input stream type is not supported",
        ),
        cpal::DefaultStreamConfigError::BackendSpecific { err } => audio_error(
            AudioInputErrorKind::BackendUnavailable,
            format!("CPAL backend error: {err}"),
        ),
    }
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
        AudioFrameFormat, AudioInputBackend, AudioInputConfig, AudioSampleFormat, CaptureEvent,
        CaptureSessionId, CaptureSessionStatus, mono_from_i16, mono_from_u16,
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
    fn interleaved_integer_frames_convert_to_mono_f32() {
        assert_eq!(
            mono_from_i16(&[i16::MAX, 0, 0, i16::MAX], 2),
            vec![0.5, 0.5]
        );
        assert_eq!(mono_from_u16(&[u16::MAX, 32768], 2).len(), 1);
    }
}
