//! `agent_voice` is a SIP-to-OpenAI voice bridge for agent workflows.
//!
//! The crate provides:
//!
//! - SIP call control and media orchestration
//! - telephony audio encoding and resampling helpers
//! - local sherpa-onnx speech backends managed through uv
//! - OpenAI STT, TTS, and responses integrations
//! - a persistent caller phone book
//! - per-call accounting and cost tracking
//! - an HTTP control API for local agents

/// OpenAI token, pricing, and per-call accounting support.
pub mod accounting;
/// Axum routes for the local control API.
pub mod api;
/// Telephony audio encoding, decoding, and resampling helpers.
pub mod audio;
/// Application configuration loading and environment overrides.
pub mod config;
/// Standalone LLM backend dispatch and provider selection.
pub mod llm;
/// OpenAI API client logic and prompt orchestration.
pub mod openai;
/// Persistent caller phone-book storage and validation helpers.
pub mod phonebook;
/// SIP call orchestration and runtime service state.
pub mod service;
/// Local sherpa-onnx speech bridge support.
pub mod sherpa_onnx;
/// Runtime speech backend dispatch and provider selection.
pub mod speech;
/// Runtime speech-to-text backend dispatch and provider selection.
pub mod stt;
/// Runtime text-to-speech backend dispatch and provider selection.
pub mod tts;
/// Runtime unified voice-model dispatch and provider selection.
pub mod voice;
