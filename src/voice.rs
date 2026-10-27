//! Unified voice-model dispatch for OpenAI audio chat completion models.

use anyhow::{Result, anyhow};

use crate::config::{OpenAiVoiceConfig, VoiceConfig, VoiceProvider, VoiceTransport};
use crate::openai::{OpenAiClients, TranscriptEvent, VoiceResponseResult};
use crate::realtime_voice::{RealtimeAdapter, RealtimeConnectionConfig};

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
        let openai_api_key = openai.config().api_key().to_string();
        let openai_config = config.openai.clone();
        let xai_config = config.xai.clone();
        let backend = match config.provider {
            VoiceProvider::Disabled => VoiceBackend::Disabled,
            VoiceProvider::OpenAi => {
                let realtime = if openai_config.transport == VoiceTransport::Realtime {
                    Some(RealtimeAdapter::OpenAi(RealtimeConnectionConfig {
                        endpoint: openai_config.realtime_url.clone(),
                        api_key: openai_api_key,
                        model: openai_config.model.clone(),
                        voice: openai_config.voice.clone(),
                        instructions: openai_config.instructions.clone(),
                        transcription_model: Some("gpt-4o-transcribe".to_string()),
                        language_hint: None,
                        reasoning_effort: None,
                    }))
                } else {
                    None
                };
                VoiceBackend::OpenAi {
                    client: Box::new(openai),
                    config: openai_config,
                    realtime,
                }
            }
            VoiceProvider::Xai => VoiceBackend::Xai {
                realtime: RealtimeAdapter::Xai(RealtimeConnectionConfig {
                    endpoint: xai_config.realtime_url.clone(),
                    api_key: xai_config.api_key().to_string(),
                    model: xai_config.model.clone(),
                    voice: xai_config.voice.clone(),
                    instructions: xai_config.instructions.clone(),
                    transcription_model: xai_config.transcription_model.clone(),
                    language_hint: xai_config.language_hint.clone(),
                    reasoning_effort: Some(xai_config.reasoning_effort.trim().to_string()),
                }),
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
            VoiceBackend::Xai { .. } => "xai",
            VoiceBackend::Disabled => "disabled",
        }
    }

    /// Returns the configured voice-model label when enabled.
    pub fn model_name(&self) -> Option<String> {
        match &self.backend {
            VoiceBackend::OpenAi { config, .. } => Some(config.model.clone()),
            VoiceBackend::Xai { realtime } => Some(realtime.model().to_string()),
            VoiceBackend::Disabled => None,
        }
    }

    /// Returns whether the selected voice backend should use STT before generating a response.
    pub fn requires_inbound_transcription(&self) -> bool {
        match &self.backend {
            VoiceBackend::OpenAi { config, .. } => {
                config.transport == VoiceTransport::ChatCompletions
                    && config.input_transcription_model.is_some()
            }
            VoiceBackend::Xai { .. } => false,
            VoiceBackend::Disabled => false,
        }
    }

    /// Returns true when calls use a full-duplex realtime websocket.
    pub fn is_realtime(&self) -> bool {
        match &self.backend {
            VoiceBackend::OpenAi { realtime, .. } => realtime.is_some(),
            VoiceBackend::Xai { .. } => true,
            VoiceBackend::Disabled => false,
        }
    }

    /// Returns the realtime adapter for a full-duplex call.
    pub fn realtime_adapter(&self) -> Option<RealtimeAdapter> {
        match &self.backend {
            VoiceBackend::OpenAi { realtime, .. } => realtime.clone(),
            VoiceBackend::Xai { realtime } => Some(realtime.clone()),
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
            VoiceBackend::OpenAi {
                client,
                config,
                realtime,
            } => {
                if realtime.is_some() {
                    return Err(anyhow!(
                        "realtime voice calls must use the streaming bridge"
                    ));
                }
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
            VoiceBackend::Xai { .. } => {
                Err(anyhow!("xAI realtime calls must use the streaming bridge"))
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
        realtime: Option<RealtimeAdapter>,
    },
    Xai {
        realtime: RealtimeAdapter,
    },
    Disabled,
}
