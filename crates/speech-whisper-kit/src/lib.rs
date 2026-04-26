#![forbid(unsafe_code)]

use std::{
    io::{self, BufReader},
    path::{Path, PathBuf},
};

use omne_fs_primitives::{File as CapFile, MissingRootPolicy, open_regular_file_at, open_root};
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
    require_regular_nofollow_file(&config.model_path, "model file")
        .map_err(|message| WhisperRuntimeError::ModelUnavailable(message.to_string()))?;
    require_regular_nofollow_file(&input.audio_path, "audio file")
        .map_err(|message| WhisperRuntimeError::InvalidRequest(message.to_string()))?;

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
    let file = open_regular_nofollow_file(path, "audio file").map_err(|error| {
        WhisperRuntimeError::UnsupportedAudioFormat(format!(
            "failed to open PCM WAV {}: {error}",
            path.display()
        ))
    })?;
    let reader = hound::WavReader::new(BufReader::new(file)).map_err(|error| {
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

fn require_regular_nofollow_file(path: &Path, label: &str) -> Result<(), String> {
    open_regular_nofollow_file(path, label)
        .map(drop)
        .map_err(|error| format!("{label} must be a regular non-symlink file: {error}"))
}

fn open_regular_nofollow_file(path: &Path, label: &str) -> io::Result<CapFile> {
    let leaf = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} path must include a file name: {}", path.display()),
        )
    })?;
    let parent = match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let root = open_root(
        &parent,
        label,
        MissingRootPolicy::Error,
        |_, _, _, error| error,
    )?
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{label} parent directory not found: {}", path.display()),
        )
    })?;
    open_regular_file_at(root.dir(), Path::new(leaf))
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

    use super::{
        WhisperRsRuntimeConfig, WhisperRuntimeError, read_pcm_wav_as_mono_f32,
        require_regular_nofollow_file,
    };

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

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_wav_input() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let target = temp.path().join("target.wav");
        let link = temp.path().join("sample.wav");
        write_test_wav(&target, 16_000, 1, &[0, 1]).expect("write wav");
        symlink(&target, &link).expect("create wav symlink");

        let error = read_pcm_wav_as_mono_f32(&link).expect_err("symlinked wav should fail");

        assert!(matches!(
            error,
            WhisperRuntimeError::UnsupportedAudioFormat(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_model_file() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let target = temp.path().join("model.bin");
        let link = temp.path().join("linked-model.bin");
        std::fs::write(&target, b"model").expect("write model");
        symlink(&target, &link).expect("create model symlink");

        let error = require_regular_nofollow_file(&link, "model file")
            .expect_err("symlinked model should fail");

        assert!(error.contains("regular non-symlink file"));
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
