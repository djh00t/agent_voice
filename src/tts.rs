//! Text-to-speech backend dispatch for OpenAI and local sherpa-onnx runtimes.

use anyhow::Result;

use crate::accounting::TokenUsage;
use crate::config::{SpeechConfig, SpeechProvider};
use crate::openai::OpenAiClients;
use crate::sherpa_onnx::{SherpaOnnxClient, SherpaOnnxSynthesis};

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
/// The configured TTS service for the current runtime.
pub struct TtsService {
    backend: TtsBackend,
}

impl TtsService {
    /// Builds the TTS backend from the resolved app configuration.
    pub fn new(speech: SpeechConfig, openai: OpenAiClients, sherpa: SherpaOnnxClient) -> Self {
        let backend = match speech.tts_provider {
            SpeechProvider::OpenAi => TtsBackend::OpenAi(openai),
            SpeechProvider::SherpaOnnx => TtsBackend::SherpaOnnx(sherpa),
        };
        Self { backend }
    }

    /// Returns the currently configured TTS backend label.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            TtsBackend::OpenAi(_) => "openai",
            TtsBackend::SherpaOnnx(_) => "sherpa-onnx",
        }
    }

    /// Returns the currently configured TTS model label.
    pub fn model_name(&self) -> String {
        match &self.backend {
            TtsBackend::OpenAi(client) => client.config().tts_model.clone(),
            TtsBackend::SherpaOnnx(client) => client.tts_model_name(),
        }
    }

    /// Synthesizes text using the configured backend.
    pub async fn speak_text(
        &self,
        text: &str,
        voice_override: Option<String>,
        instructions: Option<String>,
    ) -> Result<SynthesisOutcome> {
        match &self.backend {
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
enum TtsBackend {
    OpenAi(OpenAiClients),
    SherpaOnnx(SherpaOnnxClient),
}
