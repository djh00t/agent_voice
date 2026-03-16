//! Local sherpa-onnx speech bridge for offline STT and TTS.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, info};

use crate::audio::{TELEPHONY_RATE, resample_linear_mono};
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

/// Bridge client that invokes the uv-managed Python sherpa-onnx runtime.
#[derive(Clone)]
pub struct SherpaOnnxClient {
    config: SherpaOnnxConfig,
    runtime: Arc<SherpaOnnxRuntime>,
}

impl SherpaOnnxClient {
    /// Creates persistent local sherpa-onnx workers from validated runtime config.
    pub async fn new(config: SherpaOnnxConfig, enable_stt: bool, enable_tts: bool) -> Result<Self> {
        let stt = if enable_stt {
            Some(Arc::new(
                PersistentWorker::spawn(WorkerMode::Stt, config.clone()).await?,
            ))
        } else {
            None
        };
        let tts = if enable_tts {
            Some(Arc::new(
                PersistentWorker::spawn(WorkerMode::Tts, config.clone()).await?,
            ))
        } else {
            None
        };
        Ok(Self {
            config,
            runtime: Arc::new(SherpaOnnxRuntime { stt, tts }),
        })
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
        let worker = self
            .runtime
            .stt
            .as_ref()
            .ok_or_else(|| anyhow!("local sherpa-onnx STT worker is not configured"))?;
        let payload: SttPayload = worker
            .request(&SttRequest {
                wav_b64: BASE64_STANDARD.encode(wav_bytes),
            })
            .await?;
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
        let worker = self
            .runtime
            .tts
            .as_ref()
            .ok_or_else(|| anyhow!("local sherpa-onnx TTS worker is not configured"))?;
        let speaker_id = resolve_speaker_id(voice_override.as_deref(), self.config.tts.speaker_id)?;
        let payload: TtsPayload = worker
            .request(&TtsRequest {
                text: text.to_string(),
                speaker_id,
            })
            .await?;
        let generated_pcm = decode_pcm_s16le(&payload.pcm_s16le_b64)?;
        let telephony_pcm =
            resample_linear_mono(&generated_pcm, payload.sample_rate, TELEPHONY_RATE);
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
}

struct SherpaOnnxRuntime {
    stt: Option<Arc<PersistentWorker>>,
    tts: Option<Arc<PersistentWorker>>,
}

struct PersistentWorker {
    mode: WorkerMode,
    request_timeout: Duration,
    inner: Mutex<BridgeProcess>,
}

impl PersistentWorker {
    async fn spawn(mode: WorkerMode, config: SherpaOnnxConfig) -> Result<Self> {
        let process = BridgeProcess::spawn(mode, &config).await?;
        Ok(Self {
            mode,
            request_timeout: Duration::from_millis(config.request_timeout_ms),
            inner: Mutex::new(process),
        })
    }

    async fn request<Req, Res>(&self, request: &Req) -> Result<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let mut process = self.inner.lock().await;
        process
            .request(self.mode, request, self.request_timeout)
            .await
    }
}

struct BridgeProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl BridgeProcess {
    async fn spawn(mode: WorkerMode, config: &SherpaOnnxConfig) -> Result<Self> {
        let mut command = Command::new(&config.python_bin);
        command
            .arg("-u")
            .arg(&config.bridge_script)
            .arg(mode.serve_command())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("PYTHONUNBUFFERED", "1");

        for argument in mode.bridge_args(config)? {
            command.arg(argument);
        }
        if config.warmup_on_startup {
            command.arg("--warmup");
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start sherpa-onnx {} worker", mode.label()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("sherpa-onnx {} worker stdin unavailable", mode.label()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("sherpa-onnx {} worker stdout unavailable", mode.label()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("sherpa-onnx {} worker stderr unavailable", mode.label()))?;
        spawn_stderr_logger(mode, stderr);

        let mut stdout = BufReader::new(stdout);
        let mut ready_line = String::new();
        let startup_timeout = Duration::from_millis(config.startup_timeout_ms);
        let bytes_read = timeout(startup_timeout, stdout.read_line(&mut ready_line))
            .await
            .with_context(|| {
                format!(
                    "timed out waiting for sherpa-onnx {} worker readiness",
                    mode.label()
                )
            })?
            .with_context(|| {
                format!(
                    "failed to read sherpa-onnx {} worker readiness",
                    mode.label()
                )
            })?;
        if bytes_read == 0 {
            let status = child.wait().await.with_context(|| {
                format!("failed to wait for sherpa-onnx {} worker", mode.label())
            })?;
            bail!(
                "sherpa-onnx {} worker exited before readiness with status {}",
                mode.label(),
                status
            );
        }

        let ready: ReadyPayload = serde_json::from_str(ready_line.trim())
            .with_context(|| format!("invalid sherpa-onnx {} readiness payload", mode.label()))?;
        if !ready.ok {
            bail!(
                "sherpa-onnx {} worker failed to initialize: {}",
                mode.label(),
                ready
                    .error
                    .unwrap_or_else(|| "unknown initialization failure".to_string())
            );
        }

        info!(
            mode = mode.label(),
            model = %ready.model,
            load_ms = ready.load_ms,
            warmup_ms = ready.warmup_ms,
            warmup_on_startup = config.warmup_on_startup,
            "persistent sherpa-onnx worker ready"
        );

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout,
        })
    }

    async fn request<Req, Res>(
        &mut self,
        mode: WorkerMode,
        request: &Req,
        request_timeout: Duration,
    ) -> Result<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let request_json = serde_json::to_string(request)
            .with_context(|| format!("failed to encode sherpa-onnx {} request", mode.label()))?;
        timeout(
            request_timeout,
            self.stdin.write_all(request_json.as_bytes()),
        )
        .await
        .with_context(|| {
            format!(
                "timed out writing sherpa-onnx {} request to worker",
                mode.label()
            )
        })?
        .with_context(|| format!("failed to write sherpa-onnx {} request", mode.label()))?;
        timeout(request_timeout, self.stdin.write_all(b"\n"))
            .await
            .with_context(|| {
                format!(
                    "timed out terminating sherpa-onnx {} request line",
                    mode.label()
                )
            })?
            .with_context(|| format!("failed to terminate sherpa-onnx {} request", mode.label()))?;
        timeout(request_timeout, self.stdin.flush())
            .await
            .with_context(|| format!("timed out flushing sherpa-onnx {} request", mode.label()))?
            .with_context(|| format!("failed to flush sherpa-onnx {} request", mode.label()))?;

        let mut response_line = String::new();
        let bytes_read = timeout(request_timeout, self.stdout.read_line(&mut response_line))
            .await
            .with_context(|| {
                format!(
                    "timed out waiting for sherpa-onnx {} response",
                    mode.label()
                )
            })?
            .with_context(|| format!("failed to read sherpa-onnx {} response", mode.label()))?;
        if bytes_read == 0 {
            let status = self.child.wait().await.with_context(|| {
                format!("failed to wait for sherpa-onnx {} worker", mode.label())
            })?;
            bail!(
                "sherpa-onnx {} worker exited during request with status {}",
                mode.label(),
                status
            );
        }

        let envelope: BridgeEnvelope<Res> = serde_json::from_str(response_line.trim())
            .with_context(|| format!("invalid sherpa-onnx {} response payload", mode.label()))?;
        if !envelope.ok {
            bail!(
                "sherpa-onnx {} request failed: {}",
                mode.label(),
                envelope
                    .error
                    .unwrap_or_else(|| "unknown worker error".to_string())
            );
        }

        envelope.result.ok_or_else(|| {
            anyhow!(
                "sherpa-onnx {} response missing result payload",
                mode.label()
            )
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum WorkerMode {
    Stt,
    Tts,
}

impl WorkerMode {
    fn label(self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::Tts => "tts",
        }
    }

    fn serve_command(self) -> &'static str {
        match self {
            Self::Stt => "serve-stt",
            Self::Tts => "serve-tts",
        }
    }

    fn bridge_args(self, config: &SherpaOnnxConfig) -> Result<Vec<String>> {
        match self {
            Self::Stt => build_stt_process_args(config),
            Self::Tts => build_tts_process_args(config),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReadyPayload {
    ok: bool,
    #[serde(default)]
    model: String,
    #[serde(default)]
    load_ms: u128,
    #[serde(default)]
    warmup_ms: u128,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BridgeEnvelope<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SttRequest {
    wav_b64: String,
}

#[derive(Debug, Deserialize)]
struct SttPayload {
    text: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Serialize)]
struct TtsRequest {
    text: String,
    speaker_id: u32,
}

#[derive(Debug, Deserialize)]
struct TtsPayload {
    sample_rate: u32,
    sample_count: usize,
    pcm_s16le_b64: String,
}

fn build_stt_process_args(config: &SherpaOnnxConfig) -> Result<Vec<String>> {
    let mut args = vec![
        "--model-family".to_string(),
        config.stt.model_family.clone(),
        "--provider".to_string(),
        config.provider.clone(),
        "--num-threads".to_string(),
        config.num_threads.to_string(),
    ];
    if config.debug {
        args.push("--debug".to_string());
    }

    match normalized_family(&config.stt.model_family).as_str() {
        "moonshine" | "moonshine_v1" => {
            args.extend([
                "--moonshine-version".to_string(),
                "v1".to_string(),
                "--moonshine-preprocessor".to_string(),
                config.stt.moonshine.preprocessor.clone(),
                "--moonshine-encoder".to_string(),
                config.stt.moonshine.encoder.clone(),
                "--moonshine-uncached-decoder".to_string(),
                config.stt.moonshine.uncached_decoder.clone(),
                "--moonshine-cached-decoder".to_string(),
                config.stt.moonshine.cached_decoder.clone(),
                "--moonshine-tokens".to_string(),
                config.stt.moonshine.tokens.clone(),
            ]);
        }
        "moonshine_v2" => {
            args.extend([
                "--moonshine-version".to_string(),
                "v2".to_string(),
                "--moonshine-encoder".to_string(),
                config.stt.moonshine.encoder.clone(),
                "--moonshine-decoder".to_string(),
                config.stt.moonshine.decoder.clone(),
                "--moonshine-tokens".to_string(),
                config.stt.moonshine.tokens.clone(),
            ]);
        }
        other => bail!("unsupported sherpa-onnx STT model family {}", other),
    }

    Ok(args)
}

fn build_tts_process_args(config: &SherpaOnnxConfig) -> Result<Vec<String>> {
    let mut args = vec![
        "--model-family".to_string(),
        config.tts.model_family.clone(),
        "--provider".to_string(),
        config.provider.clone(),
        "--num-threads".to_string(),
        config.num_threads.to_string(),
        "--speed".to_string(),
        config.tts.speed.to_string(),
        "--speaker-id".to_string(),
        config.tts.speaker_id.to_string(),
    ];
    if config.debug {
        args.push("--debug".to_string());
    }

    match normalized_family(&config.tts.model_family).as_str() {
        "kokoro" => {
            args.extend([
                "--kokoro-model".to_string(),
                config.tts.kokoro.model.clone(),
                "--kokoro-voices".to_string(),
                config.tts.kokoro.voices.clone(),
                "--kokoro-tokens".to_string(),
                config.tts.kokoro.tokens.clone(),
                "--kokoro-data-dir".to_string(),
                config.tts.kokoro.data_dir.clone(),
            ]);
            if !config.tts.kokoro.lexicon.is_empty() {
                args.extend([
                    "--kokoro-lexicon".to_string(),
                    config.tts.kokoro.lexicon.clone(),
                ]);
            }
            if !config.tts.kokoro.dict_dir.is_empty() {
                args.extend([
                    "--kokoro-dict-dir".to_string(),
                    config.tts.kokoro.dict_dir.clone(),
                ]);
            }
            if !config.tts.kokoro.lang.is_empty() {
                args.extend(["--kokoro-lang".to_string(), config.tts.kokoro.lang.clone()]);
            }
        }
        other => bail!("unsupported sherpa-onnx TTS model family {}", other),
    }

    Ok(args)
}

fn spawn_stderr_logger(mode: WorkerMode, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                debug!(mode = mode.label(), line = %line, "sherpa-onnx worker stderr");
            }
        }
    });
}

fn decode_pcm_s16le(encoded: &str) -> Result<Vec<i16>> {
    let bytes = BASE64_STANDARD
        .decode(encoded)
        .context("failed to decode sherpa-onnx PCM payload")?;
    if bytes.len() % 2 != 0 {
        bail!("invalid sherpa-onnx PCM payload length {}", bytes.len());
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
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

    #[tokio::test]
    async fn stt_model_name_reflects_moonshine_v2() {
        let client = SherpaOnnxClient::new(
            SherpaOnnxConfig {
                stt: SherpaOnnxSttConfig {
                    model_family: "moonshine_v2".to_string(),
                    ..SherpaOnnxSttConfig::default()
                },
                ..SherpaOnnxConfig::default()
            },
            false,
            false,
        )
        .await
        .expect("client without workers");
        assert_eq!(client.stt_model_name(), "sherpa-onnx-moonshine-v2");
    }

    #[tokio::test]
    async fn tts_model_name_reflects_family() {
        let client = SherpaOnnxClient::new(
            SherpaOnnxConfig {
                tts: SherpaOnnxTtsConfig {
                    model_family: "kokoro".to_string(),
                    kokoro: SherpaOnnxKokoroConfig::default(),
                    ..SherpaOnnxTtsConfig::default()
                },
                ..SherpaOnnxConfig::default()
            },
            false,
            false,
        )
        .await
        .expect("client without workers");
        assert_eq!(client.tts_model_name(), "sherpa-onnx-kokoro");
    }

    #[test]
    fn speaker_override_must_be_numeric() {
        let error = resolve_speaker_id(Some("af_bella"), 0).expect_err("numeric failure");
        assert!(error.to_string().contains("numeric"));
    }

    #[test]
    fn decode_pcm_payload_preserves_samples() {
        let encoded =
            BASE64_STANDARD.encode([0_u8, 0_u8, 255_u8, 127_u8, 0_u8, 128_u8, 52_u8, 18_u8]);
        let samples = decode_pcm_s16le(&encoded).expect("valid PCM");
        assert_eq!(samples, vec![0, i16::MAX, i16::MIN, 0x1234]);
    }
}
