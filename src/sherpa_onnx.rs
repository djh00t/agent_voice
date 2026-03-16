//! Local sherpa-onnx speech bridge for offline STT and TTS.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use tokio::process::Command;
use tracing::debug;
use uuid::Uuid;

use crate::audio::{TELEPHONY_RATE, decode_wav_mono_i16, resample_linear_mono};
use crate::config::SherpaOnnxConfig;

/// Local offline transcription result produced by sherpa-onnx.
#[derive(Debug, Clone)]
pub struct SherpaOnnxTranscription {
    pub text: String,
    pub language: Option<String>,
    pub model: String,
}

/// Local offline TTS result produced by sherpa-onnx.
#[derive(Debug, Clone)]
pub struct SherpaOnnxSynthesis {
    pub pcm: Vec<i16>,
    pub model: String,
    pub sample_count: usize,
}

#[derive(Debug, Clone)]
/// Bridge client that invokes the uv-managed Python sherpa-onnx runtime.
pub struct SherpaOnnxClient {
    config: SherpaOnnxConfig,
}

impl SherpaOnnxClient {
    /// Creates a new local sherpa-onnx client from validated runtime config.
    pub fn new(config: SherpaOnnxConfig) -> Self {
        Self { config }
    }

    /// Returns the configured STT model label for logging and accounting.
    pub fn stt_model_name(&self) -> String {
        match normalized_family(&self.config.stt.model_family).as_str() {
            "moonshine" | "moonshine_v1" => "sherpa-onnx-moonshine-v1".to_string(),
            "moonshine_v2" => "sherpa-onnx-moonshine-v2".to_string(),
            other => format!("sherpa-onnx-{}", other),
        }
    }

    /// Returns the configured TTS model label for logging and accounting.
    pub fn tts_model_name(&self) -> String {
        format!(
            "sherpa-onnx-{}",
            normalized_family(&self.config.tts.model_family)
        )
    }

    /// Transcribes a WAV utterance using the configured local Moonshine model.
    pub async fn transcribe_wav(&self, wav_bytes: Vec<u8>) -> Result<SherpaOnnxTranscription> {
        let input_wav = scratch_path("sherpa-stt", "wav");
        tokio::fs::write(&input_wav, wav_bytes)
            .await
            .with_context(|| format!("failed to write {}", input_wav.display()))?;

        let output = self.run_bridge(self.build_stt_args(&input_wav)?).await;
        let cleanup = tokio::fs::remove_file(&input_wav).await;
        if let Err(error) = cleanup
            && error.kind() != std::io::ErrorKind::NotFound
        {
            debug!(path = %input_wav.display(), error = %error, "failed to clean up sherpa STT input");
        }

        let output = output?;
        let payload: SttPayload = serde_json::from_slice(&output.stdout)
            .context("failed to parse sherpa-onnx STT response")?;
        Ok(SherpaOnnxTranscription {
            text: payload.text.trim().to_string(),
            language: payload.language.filter(|value| !value.trim().is_empty()),
            model: self.stt_model_name(),
        })
    }

    /// Synthesizes text to telephony-rate mono PCM using local sherpa-onnx TTS.
    pub async fn speak_text(
        &self,
        text: &str,
        voice_override: Option<String>,
    ) -> Result<SherpaOnnxSynthesis> {
        let output_wav = scratch_path("sherpa-tts", "wav");
        let output = self
            .run_bridge(self.build_tts_args(text, voice_override.as_deref(), &output_wav)?)
            .await;
        let output = output?;
        let payload: TtsPayload = serde_json::from_slice(&output.stdout)
            .context("failed to parse sherpa-onnx TTS response")?;
        let wav_bytes = tokio::fs::read(&output_wav)
            .await
            .with_context(|| format!("failed to read {}", output_wav.display()))?;
        let cleanup = tokio::fs::remove_file(&output_wav).await;
        if let Err(error) = cleanup
            && error.kind() != std::io::ErrorKind::NotFound
        {
            debug!(path = %output_wav.display(), error = %error, "failed to clean up sherpa TTS output");
        }

        let (sample_rate, samples) = decode_wav_mono_i16(&wav_bytes)?;
        let telephony_pcm = resample_linear_mono(&samples, sample_rate, TELEPHONY_RATE);
        let sample_count = telephony_pcm.len();
        debug!(
            model = %self.tts_model_name(),
            generated_sample_rate = payload.sample_rate,
            generated_sample_count = payload.sample_count,
            telephony_sample_count = sample_count,
            "decoded sherpa-onnx TTS waveform"
        );
        Ok(SherpaOnnxSynthesis {
            pcm: telephony_pcm,
            model: self.tts_model_name(),
            sample_count,
        })
    }

    fn build_stt_args(&self, input_wav: &Path) -> Result<Vec<String>> {
        let mut args = vec![
            "stt".to_string(),
            "--model-family".to_string(),
            self.config.stt.model_family.clone(),
            "--provider".to_string(),
            self.config.provider.clone(),
            "--num-threads".to_string(),
            self.config.num_threads.to_string(),
            "--input-wav".to_string(),
            input_wav.display().to_string(),
        ];
        if self.config.debug {
            args.push("--debug".to_string());
        }

        match normalized_family(&self.config.stt.model_family).as_str() {
            "moonshine" | "moonshine_v1" => {
                args.extend([
                    "--moonshine-version".to_string(),
                    "v1".to_string(),
                    "--moonshine-preprocessor".to_string(),
                    self.config.stt.moonshine.preprocessor.clone(),
                    "--moonshine-encoder".to_string(),
                    self.config.stt.moonshine.encoder.clone(),
                    "--moonshine-uncached-decoder".to_string(),
                    self.config.stt.moonshine.uncached_decoder.clone(),
                    "--moonshine-cached-decoder".to_string(),
                    self.config.stt.moonshine.cached_decoder.clone(),
                    "--moonshine-tokens".to_string(),
                    self.config.stt.moonshine.tokens.clone(),
                ]);
            }
            "moonshine_v2" => {
                args.extend([
                    "--moonshine-version".to_string(),
                    "v2".to_string(),
                    "--moonshine-encoder".to_string(),
                    self.config.stt.moonshine.encoder.clone(),
                    "--moonshine-decoder".to_string(),
                    self.config.stt.moonshine.decoder.clone(),
                    "--moonshine-tokens".to_string(),
                    self.config.stt.moonshine.tokens.clone(),
                ]);
            }
            other => bail!("unsupported sherpa-onnx STT model family {}", other),
        }

        Ok(args)
    }

    fn build_tts_args(
        &self,
        text: &str,
        voice_override: Option<&str>,
        output_wav: &Path,
    ) -> Result<Vec<String>> {
        let speaker_id = resolve_speaker_id(voice_override, self.config.tts.speaker_id)?;
        let mut args = vec![
            "tts".to_string(),
            "--model-family".to_string(),
            self.config.tts.model_family.clone(),
            "--provider".to_string(),
            self.config.provider.clone(),
            "--num-threads".to_string(),
            self.config.num_threads.to_string(),
            "--speed".to_string(),
            self.config.tts.speed.to_string(),
            "--speaker-id".to_string(),
            speaker_id.to_string(),
            "--text".to_string(),
            text.to_string(),
            "--output-wav".to_string(),
            output_wav.display().to_string(),
        ];
        if self.config.debug {
            args.push("--debug".to_string());
        }

        match normalized_family(&self.config.tts.model_family).as_str() {
            "kokoro" => {
                args.extend([
                    "--kokoro-model".to_string(),
                    self.config.tts.kokoro.model.clone(),
                    "--kokoro-voices".to_string(),
                    self.config.tts.kokoro.voices.clone(),
                    "--kokoro-tokens".to_string(),
                    self.config.tts.kokoro.tokens.clone(),
                    "--kokoro-data-dir".to_string(),
                    self.config.tts.kokoro.data_dir.clone(),
                ]);
                if !self.config.tts.kokoro.lexicon.is_empty() {
                    args.extend([
                        "--kokoro-lexicon".to_string(),
                        self.config.tts.kokoro.lexicon.clone(),
                    ]);
                }
                if !self.config.tts.kokoro.dict_dir.is_empty() {
                    args.extend([
                        "--kokoro-dict-dir".to_string(),
                        self.config.tts.kokoro.dict_dir.clone(),
                    ]);
                }
                if !self.config.tts.kokoro.lang.is_empty() {
                    args.extend([
                        "--kokoro-lang".to_string(),
                        self.config.tts.kokoro.lang.clone(),
                    ]);
                }
            }
            other => bail!("unsupported sherpa-onnx TTS model family {}", other),
        }

        Ok(args)
    }

    async fn run_bridge(&self, args: Vec<String>) -> Result<std::process::Output> {
        debug!(
            python_bin = %self.config.python_bin,
            bridge_script = %self.config.bridge_script,
            args = ?args,
            "invoking sherpa-onnx bridge"
        );
        let output = Command::new(&self.config.python_bin)
            .arg(&self.config.bridge_script)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .await
            .with_context(|| {
                format!(
                    "failed to start sherpa-onnx bridge {} {}",
                    self.config.python_bin, self.config.bridge_script
                )
            })?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "sherpa-onnx bridge failed with status {}: stdout={} stderr={}",
                output.status,
                stdout.trim(),
                stderr.trim(),
            ));
        }
        Ok(output)
    }
}

#[derive(Debug, Deserialize)]
struct SttPayload {
    text: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TtsPayload {
    sample_rate: u32,
    sample_count: usize,
}

fn scratch_path(prefix: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}-{}.{}", prefix, Uuid::new_v4(), extension))
}

fn normalized_family(value: &str) -> String {
    value.trim().replace('-', "_").to_ascii_lowercase()
}

fn resolve_speaker_id(voice_override: Option<&str>, default_speaker_id: u32) -> Result<u32> {
    match voice_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value.parse::<u32>().with_context(|| {
            format!(
                "local sherpa-onnx TTS voice override must be numeric, got {}",
                value
            )
        }),
        None => Ok(default_speaker_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SherpaOnnxKokoroConfig;
    use crate::config::SherpaOnnxSttConfig;
    use crate::config::SherpaOnnxTtsConfig;

    #[test]
    fn stt_model_name_reflects_moonshine_v2() {
        let client = SherpaOnnxClient::new(SherpaOnnxConfig {
            stt: SherpaOnnxSttConfig {
                model_family: "moonshine_v2".to_string(),
                ..SherpaOnnxSttConfig::default()
            },
            ..SherpaOnnxConfig::default()
        });
        assert_eq!(client.stt_model_name(), "sherpa-onnx-moonshine-v2");
    }

    #[test]
    fn tts_model_name_reflects_family() {
        let client = SherpaOnnxClient::new(SherpaOnnxConfig {
            tts: SherpaOnnxTtsConfig {
                model_family: "kokoro".to_string(),
                kokoro: SherpaOnnxKokoroConfig::default(),
                ..SherpaOnnxTtsConfig::default()
            },
            ..SherpaOnnxConfig::default()
        });
        assert_eq!(client.tts_model_name(), "sherpa-onnx-kokoro");
    }

    #[test]
    fn speaker_override_must_be_numeric() {
        let error = resolve_speaker_id(Some("af_bella"), 0).expect_err("numeric failure");
        assert!(error.to_string().contains("numeric"));
    }
}
