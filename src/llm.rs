//! Standalone LLM backend dispatch for assistant replies and caller extraction.

use anyhow::{Result, anyhow};

use crate::config::{LlmConfig, LlmProvider};
use crate::openai::{
    CallerUpdateResult, ConversationContext, OpenAiClients, ResponseResult, TranscriptEvent,
};

#[derive(Clone)]
/// The configured LLM service for the current runtime.
pub struct LlmService {
    backend: LlmBackend,
}

impl LlmService {
    /// Builds the LLM backend from the resolved app configuration.
    pub fn new(config: LlmConfig, openai: OpenAiClients) -> Self {
        let backend = match config.provider {
            LlmProvider::OpenAi => LlmBackend::OpenAi(openai),
            LlmProvider::None => LlmBackend::Disabled,
        };
        Self { backend }
    }

    /// Returns true when a standalone LLM backend is enabled.
    pub fn is_enabled(&self) -> bool {
        !matches!(self.backend, LlmBackend::Disabled)
    }

    /// Returns the currently configured LLM backend label.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            LlmBackend::OpenAi(_) => "openai",
            LlmBackend::Disabled => "none",
        }
    }

    /// Returns the currently configured LLM model label when enabled.
    pub fn model_name(&self) -> Option<String> {
        match &self.backend {
            LlmBackend::OpenAi(client) => Some(client.config().response_model.clone()),
            LlmBackend::Disabled => None,
        }
    }

    /// Returns the configured LLM endpoint when enabled.
    pub fn endpoint(&self) -> Option<String> {
        match &self.backend {
            LlmBackend::OpenAi(client) => Some(client.config().responses_api_url.clone()),
            LlmBackend::Disabled => None,
        }
    }

    /// Generates an assistant response using the configured backend.
    pub async fn generate_response_with_context(
        &self,
        transcript: &[TranscriptEvent],
        context: &ConversationContext,
        previous_response_id: Option<&str>,
    ) -> Result<ResponseResult> {
        match &self.backend {
            LlmBackend::OpenAi(client) => {
                client
                    .generate_response_with_context(transcript, context, previous_response_id)
                    .await
            }
            LlmBackend::Disabled => Err(anyhow!("standalone llm backend is disabled")),
        }
    }

    /// Extracts caller profile fields from transcript history when enabled.
    pub async fn extract_caller_update(
        &self,
        transcript: &[TranscriptEvent],
        caller: Option<&crate::phonebook::CallerRecord>,
    ) -> Result<CallerUpdateResult> {
        match &self.backend {
            LlmBackend::OpenAi(client) => client.extract_caller_update(transcript, caller).await,
            LlmBackend::Disabled => Err(anyhow!("standalone llm backend is disabled")),
        }
    }
}

#[derive(Clone)]
enum LlmBackend {
    OpenAi(OpenAiClients),
    Disabled,
}
