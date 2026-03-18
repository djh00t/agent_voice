//! Speech-to-text backend dispatch for OpenAI and local sherpa-onnx runtimes.

use anyhow::Result;

use crate::accounting::TokenUsage;
use crate::config::{SpeechConfig, SpeechProvider};
use crate::openai::OpenAiClients;
use crate::sherpa_onnx::{SherpaOnnxClient, SherpaOnnxTranscription};

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

#[derive(Clone)]
/// The configured STT service for the current runtime.
pub struct SttService {
    backend: SttBackend,
}

impl SttService {
    /// Builds the STT backend from the resolved app configuration.
    pub fn new(speech: SpeechConfig, openai: OpenAiClients, sherpa: SherpaOnnxClient) -> Self {
        let backend = match speech.stt_provider {
            SpeechProvider::OpenAi => SttBackend::OpenAi(openai),
            SpeechProvider::SherpaOnnx => SttBackend::SherpaOnnx(sherpa),
        };
        Self { backend }
    }

    /// Returns the currently configured STT backend label.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            SttBackend::OpenAi(_) => "openai",
            SttBackend::SherpaOnnx(_) => "sherpa-onnx",
        }
    }

    /// Returns the currently configured STT model label.
    pub fn model_name(&self) -> String {
        match &self.backend {
            SttBackend::OpenAi(client) => client.config().transcription_model.clone(),
            SttBackend::SherpaOnnx(client) => client.stt_model_name(),
        }
    }

    /// Transcribes a WAV utterance using the configured backend.
    pub async fn transcribe_wav(&self, wav_bytes: Vec<u8>) -> Result<TranscriptionOutcome> {
        match &self.backend {
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
}

#[derive(Clone)]
enum SttBackend {
    OpenAi(OpenAiClients),
    SherpaOnnx(SherpaOnnxClient),
}
