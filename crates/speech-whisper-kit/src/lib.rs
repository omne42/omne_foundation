#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperRsRuntimeConfig {
    pub model_path: PathBuf,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub n_threads: Option<i32>,
}

impl WhisperRsRuntimeConfig {
    pub fn normalized_threads(&self) -> i32 {
        self.n_threads
            .filter(|threads| *threads > 0)
            .unwrap_or_else(default_thread_count)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperTranscriptionInput {
    pub audio_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperTranscriptionOutput {
    pub text: String,
}

#[derive(Debug, Error)]
pub enum WhisperRuntimeError {
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),
    #[error("unsupported audio format: {0}")]
    UnsupportedAudioFormat(String),
    #[error("runtime unavailable: {0}")]
    RuntimeUnavailable(String),
    #[error("runtime rejected request: {0}")]
    RuntimeRejected(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

pub fn transcribe_wav(
    config: &WhisperRsRuntimeConfig,
    input: &WhisperTranscriptionInput,
) -> Result<WhisperTranscriptionOutput, WhisperRuntimeError> {
    if !config.model_path.is_file() {
        return Err(WhisperRuntimeError::ModelUnavailable(format!(
            "model file does not exist: {}",
            config.model_path.display()
        )));
    }

    if !input.audio_path.is_file() {
        return Err(WhisperRuntimeError::InvalidRequest(format!(
            "audio file does not exist: {}",
            input.audio_path.display()
        )));
    }

    let audio = read_pcm_wav_as_mono_f32(&input.audio_path)?;
    let context =
        WhisperContext::new_with_params(&config.model_path, WhisperContextParameters::default())
            .map_err(|error| {
                WhisperRuntimeError::ModelUnavailable(format!(
                    "failed to load Whisper model: {error}"
                ))
            })?;
    let mut state = context.create_state().map_err(|error| {
        WhisperRuntimeError::RuntimeUnavailable(format!("failed to create Whisper state: {error}"))
    })?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(config.normalized_threads());
    params.set_translate(false);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    match config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(language) if language.contains('\0') => {
            return Err(WhisperRuntimeError::InvalidRequest(
                "language contains a null byte".to_string(),
            ));
        }
        Some(language) => params.set_language(Some(language)),
        None => {
            params.set_detect_language(true);
            params.set_language(None);
        }
    }

    if let Some(prompt) = config
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if prompt.contains('\0') {
            return Err(WhisperRuntimeError::InvalidRequest(
                "prompt contains a null byte".to_string(),
            ));
        }
        params.set_initial_prompt(prompt);
    }

    state.full(params, &audio).map_err(|error| {
        WhisperRuntimeError::RuntimeRejected(format!("Whisper transcription failed: {error}"))
    })?;

    Ok(WhisperTranscriptionOutput {
        text: state
            .as_iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string(),
    })
}

pub fn read_pcm_wav_as_mono_f32(path: &Path) -> Result<Vec<f32>, WhisperRuntimeError> {
    let reader = hound::WavReader::open(path).map_err(|error| {
        WhisperRuntimeError::UnsupportedAudioFormat(format!(
            "failed to open PCM WAV {}: {error}",
            path.display()
        ))
    })?;
    let spec = reader.spec();

    if spec.sample_rate != 16_000 {
        return Err(WhisperRuntimeError::UnsupportedAudioFormat(format!(
            "Whisper runtime requires 16 kHz audio, got {} Hz",
            spec.sample_rate
        )));
    }

    if spec.channels == 0 || spec.channels > 2 {
        return Err(WhisperRuntimeError::UnsupportedAudioFormat(format!(
            "Whisper runtime supports mono or stereo WAV, got {} channels",
            spec.channels
        )));
    }

    let audio = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {
            let samples = reader
                .into_samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    WhisperRuntimeError::UnsupportedAudioFormat(format!(
                        "failed to read 16-bit PCM WAV samples: {error}"
                    ))
                })?;
            let mut audio = vec![0.0f32; samples.len()];
            whisper_rs::convert_integer_to_float_audio(&samples, &mut audio).map_err(|error| {
                WhisperRuntimeError::UnsupportedAudioFormat(format!(
                    "failed to convert PCM samples: {error}"
                ))
            })?;
            audio
        }
        (hound::SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                WhisperRuntimeError::UnsupportedAudioFormat(format!(
                    "failed to read 32-bit float WAV samples: {error}"
                ))
            })?,
        _ => {
            return Err(WhisperRuntimeError::UnsupportedAudioFormat(format!(
                "Whisper runtime supports PCM16 or float32 WAV, got {:?} {} bits",
                spec.sample_format, spec.bits_per_sample
            )));
        }
    };

    if spec.channels == 1 {
        return Ok(audio);
    }

    let mut mono = vec![0.0f32; audio.len() / 2];
    whisper_rs::convert_stereo_to_mono_audio(&audio, &mut mono).map_err(|error| {
        WhisperRuntimeError::UnsupportedAudioFormat(format!(
            "failed to mix stereo WAV to mono: {error}"
        ))
    })?;
    Ok(mono)
}

fn default_thread_count() -> i32 {
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(1, 8) as i32)
        .unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use hound::{SampleFormat, WavSpec, WavWriter};
    use tempfile::TempDir;

    use super::{WhisperRsRuntimeConfig, WhisperRuntimeError, read_pcm_wav_as_mono_f32};

    #[test]
    fn normalized_threads_uses_positive_override() {
        let config = WhisperRsRuntimeConfig {
            model_path: "model.bin".into(),
            language: None,
            prompt: None,
            n_threads: Some(2),
        };

        assert_eq!(config.normalized_threads(), 2);
    }

    #[test]
    fn reads_16khz_pcm_wav() {
        let temp = TempDir::new().expect("temp dir");
        let wav_path = temp.path().join("sample.wav");
        write_test_wav(&wav_path, 16_000, 1, &[0, i16::MAX]).expect("write wav");

        let samples = read_pcm_wav_as_mono_f32(&wav_path).expect("read wav");

        assert_eq!(samples.len(), 2);
        assert!(samples[1] > 0.99);
    }

    #[test]
    fn rejects_non_16khz_wav() {
        let temp = TempDir::new().expect("temp dir");
        let wav_path = temp.path().join("sample.wav");
        write_test_wav(&wav_path, 48_000, 1, &[0, 1]).expect("write wav");

        let error = read_pcm_wav_as_mono_f32(&wav_path).expect_err("unsupported sample rate");

        assert!(matches!(
            error,
            WhisperRuntimeError::UnsupportedAudioFormat(_)
        ));
    }

    fn write_test_wav(
        path: &std::path::Path,
        sample_rate: u32,
        channels: u16,
        samples: &[i16],
    ) -> Result<(), hound::Error> {
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec)?;
        for sample in samples {
            writer.write_sample(*sample)?;
        }
        writer.finalize()
    }
}
