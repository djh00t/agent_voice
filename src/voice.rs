//! Unified voice-model dispatch for OpenAI audio chat completion models.

use anyhow::{Result, anyhow};

use crate::config::{OpenAiVoiceConfig, VoiceConfig, VoiceProvider};
use crate::openai::{OpenAiClients, TranscriptEvent, VoiceResponseResult};

#[derive(Debug, Clone)]
/// Unified voice-model reply containing assistant transcript and ready-to-play PCM.
pub struct VoiceTurnOutcome {
    pub text: String,
    pub pcm: Vec<i16>,
    pub usage: crate::accounting::TokenUsage,
    pub model: String,
    pub endpoint: String,
}

#[derive(Clone)]
/// The configured unified voice-model service for the current runtime.
pub struct VoiceService {
    backend: VoiceBackend,
}

impl VoiceService {
    /// Builds the voice-model backend from the resolved app configuration.
    pub fn new(config: VoiceConfig, openai: OpenAiClients) -> Self {
        let backend = match config.provider {
            VoiceProvider::Disabled => VoiceBackend::Disabled,
            VoiceProvider::OpenAi => VoiceBackend::OpenAi {
                client: Box::new(openai),
                config: config.openai,
            },
        };
        Self { backend }
    }

    /// Returns true when a unified voice model is enabled.
    pub fn is_enabled(&self) -> bool {
        !matches!(self.backend, VoiceBackend::Disabled)
    }

    /// Returns the currently configured voice-model backend label.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            VoiceBackend::OpenAi { .. } => "openai",
            VoiceBackend::Disabled => "disabled",
        }
    }

    /// Returns the configured voice-model label when enabled.
    pub fn model_name(&self) -> Option<String> {
        match &self.backend {
            VoiceBackend::OpenAi { config, .. } => Some(config.model.clone()),
            VoiceBackend::Disabled => None,
        }
    }

    /// Sends a caller audio turn to the configured voice model.
    pub async fn respond_to_wav(
        &self,
        transcript: &[TranscriptEvent],
        caller_text: Option<&str>,
        wav_bytes: Vec<u8>,
        instructions: Option<String>,
    ) -> Result<VoiceTurnOutcome> {
        match &self.backend {
            VoiceBackend::OpenAi { client, config } => {
                let mut effective_config: OpenAiVoiceConfig = config.clone();
                if let Some(instructions) = instructions.map(|text| text.trim().to_string())
                    && !instructions.is_empty()
                {
                    effective_config.instructions = Some(instructions);
                }
                let result: VoiceResponseResult = client
                    .generate_voice_response(&effective_config, transcript, caller_text, wav_bytes)
                    .await?;
                Ok(VoiceTurnOutcome {
                    text: result.text,
                    pcm: result.pcm,
                    usage: result.usage,
                    model: effective_config.model,
                    endpoint: effective_config.api_url,
                })
            }
            VoiceBackend::Disabled => Err(anyhow!("voice-model backend is disabled")),
        }
    }
}

#[derive(Clone)]
enum VoiceBackend {
    OpenAi {
        client: Box<OpenAiClients>,
        config: OpenAiVoiceConfig,
    },
    Disabled,
}
