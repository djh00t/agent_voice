//! Speech backend dispatch for OpenAI and local sherpa-onnx runtimes.

use anyhow::Result;

use crate::accounting::{AccountingStore, TokenUsage};
use crate::config::{OpenAiConfig, SpeechConfig, SpeechProvider};
use crate::openai::OpenAiClients;
use crate::sherpa_onnx::{SherpaOnnxClient, SherpaOnnxSynthesis, SherpaOnnxTranscription};

#[derive(Debug, Clone)]
/// Unified STT result used by the call service regardless of backend.
pub struct TranscriptionOutcome {
    pub text: String,
    pub usage: TokenUsage,
    pub model: String,
    pub endpoint: String,
    pub usage_source: &'static str,
    pub estimated: bool,
    pub backend: &'static str,
    pub language: Option<String>,
}

#[derive(Debug, Clone)]
/// Unified TTS result used by the call service regardless of backend.
pub struct SynthesisOutcome {
    pub pcm: Vec<i16>,
    pub usage: TokenUsage,
    pub model: String,
    pub endpoint: String,
    pub usage_source: &'static str,
    pub estimated: bool,
    pub backend: &'static str,
}

#[derive(Clone)]
/// The configured speech services for the current runtime.
pub struct SpeechServices {
    stt: SttBackend,
    tts: TtsBackend,
}

impl SpeechServices {
    /// Builds STT and TTS backends from the resolved app configuration.
    pub fn new(speech: SpeechConfig, openai: OpenAiConfig) -> Result<Self> {
        let openai_clients = OpenAiClients::new(openai)?;
        let sherpa = SherpaOnnxClient::new(speech.sherpa_onnx.clone());
        let stt = match speech.stt_provider {
            SpeechProvider::OpenAi => SttBackend::OpenAi(openai_clients.clone()),
            SpeechProvider::SherpaOnnx => SttBackend::SherpaOnnx(sherpa.clone()),
        };
        let tts = match speech.tts_provider {
            SpeechProvider::OpenAi => TtsBackend::OpenAi(openai_clients),
            SpeechProvider::SherpaOnnx => TtsBackend::SherpaOnnx(sherpa),
        };
        Ok(Self { stt, tts })
    }

    /// Validates that any selected OpenAI speech models exist in the pricing catalog.
    pub fn validate_required_models(
        &self,
        accounting: &AccountingStore,
        openai: &OpenAiConfig,
    ) -> Result<()> {
        let mut required = vec![openai.response_model.as_str()];
        if matches!(self.stt, SttBackend::OpenAi(_)) {
            required.push(openai.transcription_model.as_str());
        }
        if matches!(self.tts, TtsBackend::OpenAi(_)) {
            required.push(openai.tts_model.as_str());
        }
        accounting.validate_required_models(required)
    }

    /// Returns the currently configured STT backend label.
    pub fn stt_backend_name(&self) -> &'static str {
        match self.stt {
            SttBackend::OpenAi(_) => "openai",
            SttBackend::SherpaOnnx(_) => "sherpa-onnx",
        }
    }

    /// Returns the currently configured TTS backend label.
    pub fn tts_backend_name(&self) -> &'static str {
        match self.tts {
            TtsBackend::OpenAi(_) => "openai",
            TtsBackend::SherpaOnnx(_) => "sherpa-onnx",
        }
    }

    /// Returns the currently configured TTS model label.
    pub fn tts_model_name(&self) -> String {
        match &self.tts {
            TtsBackend::OpenAi(client) => client.config().tts_model.clone(),
            TtsBackend::SherpaOnnx(client) => client.tts_model_name(),
        }
    }

    /// Transcribes a WAV utterance using the configured backend.
    pub async fn transcribe_wav(&self, wav_bytes: Vec<u8>) -> Result<TranscriptionOutcome> {
        match &self.stt {
            SttBackend::OpenAi(client) => {
                let result = client.transcribe_wav(wav_bytes).await?;
                Ok(TranscriptionOutcome {
                    text: result.text,
                    usage: result.usage,
                    model: client.config().transcription_model.clone(),
                    endpoint: client.config().transcription_api_url.clone(),
                    usage_source: "api",
                    estimated: false,
                    backend: "openai",
                    language: None,
                })
            }
            SttBackend::SherpaOnnx(client) => {
                let result: SherpaOnnxTranscription = client.transcribe_wav(wav_bytes).await?;
                Ok(TranscriptionOutcome {
                    text: result.text,
                    usage: TokenUsage::default(),
                    model: result.model,
                    endpoint: "local://sherpa-onnx/stt".to_string(),
                    usage_source: "local",
                    estimated: false,
                    backend: "sherpa-onnx",
                    language: result.language,
                })
            }
        }
    }

    /// Synthesizes text using the configured backend.
    pub async fn speak_text(
        &self,
        text: &str,
        voice_override: Option<String>,
        instructions: Option<String>,
    ) -> Result<SynthesisOutcome> {
        match &self.tts {
            TtsBackend::OpenAi(client) => {
                let pcm = client
                    .speak_text(text, voice_override, instructions)
                    .await?;
                Ok(SynthesisOutcome {
                    usage: TokenUsage::default(),
                    endpoint: client.config().audio_api_url.clone(),
                    model: client.config().tts_model.clone(),
                    usage_source: "estimated",
                    estimated: true,
                    backend: "openai",
                    pcm,
                })
            }
            TtsBackend::SherpaOnnx(client) => {
                let result: SherpaOnnxSynthesis = client.speak_text(text, voice_override).await?;
                Ok(SynthesisOutcome {
                    pcm: result.pcm,
                    usage: TokenUsage::default(),
                    model: result.model,
                    endpoint: "local://sherpa-onnx/tts".to_string(),
                    usage_source: "local",
                    estimated: false,
                    backend: "sherpa-onnx",
                })
            }
        }
    }
}

#[derive(Clone)]
enum SttBackend {
    OpenAi(OpenAiClients),
    SherpaOnnx(SherpaOnnxClient),
}

#[derive(Clone)]
enum TtsBackend {
    OpenAi(OpenAiClients),
    SherpaOnnx(SherpaOnnxClient),
}
