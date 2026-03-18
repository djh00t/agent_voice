//! OpenAI client integration for STT, TTS, and response generation.

use std::sync::Arc;
use std::time::Duration;
use std::env;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use http::{HeaderValue, Request};
use parking_lot::RwLock;
use reqwest::{Client, multipart};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::accounting::TokenUsage;
use crate::audio::{TELEPHONY_RATE, decode_wav_mono_i16, resample_linear_mono};
use crate::config::{OpenAiConfig, OpenAiVoiceConfig};
use crate::phonebook::{CallerRecord, CallerUpdate, editable_field_names};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A single transcript event emitted by the caller or assistant.
pub struct TranscriptEvent {
    pub role: String,
    pub kind: String,
    pub text: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Per-turn context sent to the LLM when generating assistant replies.
pub struct ConversationContext {
    pub assistant_name: String,
    pub caller_id: String,
    pub phone_book_writable: bool,
    pub time_of_day: String,
    pub known_caller: Option<CallerRecord>,
    pub missing_fields: Vec<String>,
    pub pending_email_confirmation: Option<String>,
}

#[derive(Debug, Clone)]
/// A completed transcription result plus token usage details.
pub struct TranscriptionResult {
    pub text: String,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone)]
/// A generated assistant turn and its associated usage information.
pub struct ResponseResult {
    pub text: String,
    pub end_call: bool,
    pub usage: TokenUsage,
    pub response_id: Option<String>,
}

#[derive(Debug, Clone)]
/// A completed audio-model response containing spoken audio and assistant transcript text.
pub struct VoiceResponseResult {
    pub text: String,
    pub pcm: Vec<i16>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnPlan {
    say: String,
    end_call: bool,
}

#[derive(Debug, Clone)]
/// A structured caller-profile extraction result and usage information.
pub struct CallerUpdateResult {
    pub update: CallerUpdate,
    pub usage: TokenUsage,
}

/// Sink for realtime transcript events and bridge errors.
pub trait TranscriptSink: Send + Sync {
    fn push_event(&self, event: TranscriptEvent);
    fn mark_error(&self, message: String);
}

#[derive(Clone)]
/// OpenAI HTTP and websocket clients used by the voice service.
pub struct OpenAiClients {
    config: OpenAiConfig,
    client: Client,
}

impl OpenAiClients {
    /// Builds a new OpenAI client set from the configured endpoints and models.
    pub fn new(config: OpenAiConfig) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_millis(config.http_connect_timeout_ms))
            .timeout(Duration::from_millis(config.http_timeout_ms))
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self { config, client })
    }

    /// Returns the underlying OpenAI configuration.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// Synthesizes text to mono PCM samples at the telephony sample rate.
    pub async fn speak_text(
        &self,
        text: &str,
        voice_override: Option<String>,
        instructions: Option<String>,
    ) -> Result<Vec<i16>> {
        let mut body = json!({
            "model": self.config.tts_model,
            "voice": voice_override.unwrap_or_else(|| self.config.tts_voice.clone()),
            "input": text,
            "response_format": self.config.tts_format,
        });

        if let Some(instructions) = instructions.or_else(|| self.config.tts_instructions.clone()) {
            body["instructions"] = json!(instructions);
        }

        // Validate the configured audio API URL before using it to avoid SSRF.
        let tts_url: reqwest::Url = self
            .config
            .audio_api_url
            .parse()
            .context("invalid TTS audio_api_url")?;

        if tts_url.scheme() != "https" {
            return Err(anyhow!("invalid TTS endpoint scheme: {}", tts_url.scheme()));
        }

        // Optionally restrict TTS requests to a configured set of allowed hosts.
        // If the OPENAI_AUDIO_ALLOWED_HOSTS env var is set (comma-separated list),
        // only hosts in that list will be accepted. If it is unset or empty, any
        // HTTPS host is allowed.
        if let Ok(allowed_hosts_var) = env::var("OPENAI_AUDIO_ALLOWED_HOSTS") {
            let allowed_hosts: Vec<&str> = allowed_hosts_var
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            if !allowed_hosts.is_empty() {
                let host = tts_url
                    .host_str()
                    .ok_or_else(|| anyhow!("TTS endpoint is missing host"))?;
                let host_allowed = allowed_hosts
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(host));
                if !host_allowed {
                    return Err(anyhow!("untrusted TTS endpoint host: {}", host));
                }
            }
        }

        debug!(
            endpoint = %tts_url,
            request_body = %body,
            "sending OpenAI TTS request"
        );
        let response = self
            .client
            .post(tts_url)
            .bearer_auth(self.config.api_key())
            .json(&body)
            .send()
            .await
            .context("failed to request TTS audio")?;

        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response.bytes().await.context("failed to read TTS body")?;
        debug!(
            status = %status,
            content_type = ?content_type,
            response_bytes = bytes.len(),
            "received OpenAI TTS response"
        );
        if !status.is_success() {
            if let Ok(body_text) = std::str::from_utf8(&bytes) {
                debug!(response_body = %body_text, "OpenAI TTS error body");
            }
            return Err(anyhow!("TTS request failed with status {status}"));
        }
        let (sample_rate, wav_samples) =
            decode_tts_audio(&self.config.tts_format, content_type.as_deref(), &bytes)?;
        let telephony_pcm = resample_linear_mono(&wav_samples, sample_rate, TELEPHONY_RATE);
        Ok(telephony_pcm)
    }

    /// Sends WAV audio to OpenAI transcription and returns the best text result.
    pub async fn transcribe_wav(&self, wav_bytes: Vec<u8>) -> Result<TranscriptionResult> {
        let wav_len = wav_bytes.len();
        let file_part = multipart::Part::bytes(wav_bytes)
            .file_name("utterance.wav")
            .mime_str("audio/wav")
            .context("failed to build transcription upload")?;
        let mut form = multipart::Form::new()
            .text("model", self.config.transcription_model.clone())
            .part("file", file_part);

        if let Some(prompt) = self.config.transcription_prompt.clone() {
            form = form.text("prompt", prompt);
        }
        if let Some(language) = self.config.transcription_language.clone() {
            form = form.text("language", language);
        }

        debug!(
            endpoint = %self.config.transcription_api_url,
            model = %self.config.transcription_model,
            wav_bytes = wav_len,
            prompt = ?self.config.transcription_prompt,
            language = ?self.config.transcription_language,
            "sending OpenAI transcription request"
        );
        let response = self
            .client
            .post(&self.config.transcription_api_url)
            .bearer_auth(self.config.api_key())
            .multipart(form)
            .send()
            .await
            .context("failed to request transcription")?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .context("failed to read transcription response")?;
        debug!(
            status = %status,
            response_body = %payload_text,
            "received OpenAI transcription response"
        );
        if !status.is_success() {
            return Err(anyhow!("transcription request failed with status {status}"));
        }
        let payload: serde_json::Value = serde_json::from_str(&payload_text)
            .context("failed to parse transcription response")?;
        let text = payload
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("transcription response did not contain text"))?;
        Ok(TranscriptionResult {
            text,
            usage: extract_usage(&payload),
        })
    }

    /// Generates a plain assistant response using only transcript history.
    pub async fn generate_response(&self, transcript: &[TranscriptEvent]) -> Result<String> {
        Ok(self
            .generate_response_with_context(
                transcript,
                &ConversationContext {
                    assistant_name: "Steve".to_string(),
                    caller_id: "unknown".to_string(),
                    phone_book_writable: true,
                    time_of_day: "day".to_string(),
                    known_caller: None,
                    missing_fields: Vec::new(),
                    pending_email_confirmation: None,
                },
                None,
            )
            .await?
            .text)
    }

    /// Generates a structured assistant response with full conversation context.
    pub async fn generate_response_with_context(
        &self,
        transcript: &[TranscriptEvent],
        context: &ConversationContext,
        previous_response_id: Option<&str>,
    ) -> Result<ResponseResult> {
        let input = response_input(transcript, previous_response_id);
        let instructions =
            response_instructions(self.config.response_instructions.as_deref(), context);

        let mut body = json!({
            "model": self.config.response_model,
            "input": input,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "call_turn_plan",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": {
                            "say": {
                                "type": "string"
                            },
                            "end_call": {
                                "type": "boolean"
                            }
                        },
                        "required": ["say", "end_call"],
                        "additionalProperties": false
                    }
                }
            }
        });
        body["instructions"] = json!(instructions);
        if let Some(previous_response_id) = previous_response_id {
            body["previous_response_id"] = json!(previous_response_id);
        }

        debug!(
            endpoint = %self.config.responses_api_url,
            previous_response_id = ?previous_response_id,
            input_items = body["input"].as_array().map(|items| items.len()).unwrap_or(0),
            instruction_chars = body["instructions"].as_str().map(|text| text.len()).unwrap_or(0),
            request_body = %body,
            "sending OpenAI responses request"
        );
        let response = self
            .client
            .post(&self.config.responses_api_url)
            .bearer_auth(self.config.api_key())
            .json(&body)
            .send()
            .await
            .context("failed to request agent response")?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .context("failed to read agent response body")?;
        debug!(
            status = %status,
            response_body = %payload_text,
            "received OpenAI responses reply"
        );
        if !status.is_success() {
            return Err(anyhow!(
                "agent response request failed with status {status}"
            ));
        }
        let payload: serde_json::Value =
            serde_json::from_str(&payload_text).context("failed to parse agent response body")?;
        let plan = sanitize_turn_plan(extract_turn_plan(&payload)?, transcript);
        Ok(ResponseResult {
            text: plan.say,
            end_call: plan.end_call,
            usage: extract_usage(&payload),
            response_id: extract_response_id(&payload),
        })
    }

    /// Extracts caller profile fields from the transcript for phone-book updates.
    pub async fn extract_caller_update(
        &self,
        transcript: &[TranscriptEvent],
        caller: Option<&CallerRecord>,
    ) -> Result<CallerUpdateResult> {
        let transcript_lines = transcript
            .iter()
            .filter(|event| event.role == "caller" || event.role == "assistant")
            .map(|event| format!("{}: {}", event.role, event.text))
            .collect::<Vec<_>>()
            .join("\n");
        let body = json!({
            "model": self.config.response_model,
            "instructions": contact_extraction_instructions(caller),
            "input": [
                {
                    "role": "user",
                    "content": format!("Transcript:\n{}", transcript_lines)
                }
            ]
        });

        debug!(
            endpoint = %self.config.responses_api_url,
            request_body = %body,
            "sending caller contact extraction request"
        );
        let response = self
            .client
            .post(&self.config.responses_api_url)
            .bearer_auth(self.config.api_key())
            .json(&body)
            .send()
            .await
            .context("failed to request caller contact extraction")?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .context("failed to read caller contact extraction body")?;
        debug!(
            status = %status,
            response_body = %payload_text,
            "received caller contact extraction reply"
        );
        if !status.is_success() {
            return Err(anyhow!(
                "caller contact extraction failed with status {status}"
            ));
        }
        let payload = serde_json::from_str::<serde_json::Value>(&payload_text)
            .context("failed to parse caller contact extraction response")?;
        let text = extract_response_text(&payload)?;
        Ok(CallerUpdateResult {
            update: parse_caller_update(&text)?,
            usage: extract_usage(&payload),
        })
    }

    /// Sends a caller audio turn to an OpenAI audio chat completion model.
    pub async fn generate_voice_response(
        &self,
        config: &OpenAiVoiceConfig,
        transcript: &[TranscriptEvent],
        caller_text: Option<&str>,
        wav_bytes: Vec<u8>,
    ) -> Result<VoiceResponseResult> {
        let mut messages = audio_chat_messages(transcript);
        let mut content = Vec::new();
        if let Some(caller_text) = caller_text.map(str::trim).filter(|text| !text.is_empty()) {
            content.push(json!({
                "type": "text",
                "text": caller_text,
            }));
        }
        content.push(json!({
            "type": "input_audio",
            "input_audio": {
                "data": BASE64.encode(wav_bytes),
                "format": "wav",
            }
        }));
        messages.push(json!({
            "role": "user",
            "content": content,
        }));

        let mut body = json!({
            "model": config.model,
            "modalities": ["text", "audio"],
            "audio": {
                "voice": config.voice,
                "format": "wav",
            },
            "messages": messages,
        });
        if let Some(instructions) = config.instructions.as_deref().map(str::trim)
            && !instructions.is_empty()
        {
            body["messages"]
                .as_array_mut()
                .expect("messages array")
                .insert(
                    0,
                    json!({
                        "role": "system",
                        "content": instructions,
                    }),
                );
        }

        debug!(
            endpoint = %config.api_url,
            model = %config.model,
            message_count = body["messages"].as_array().map(|items| items.len()).unwrap_or(0),
            request_body = %body,
            "sending OpenAI voice-model chat completion request"
        );
        let response = self
            .client
            .post(&config.api_url)
            .bearer_auth(self.config.api_key())
            .json(&body)
            .send()
            .await
            .context("failed to request voice-model completion")?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .context("failed to read voice-model completion body")?;
        debug!(
            status = %status,
            response_body = %payload_text,
            "received OpenAI voice-model chat completion reply"
        );
        if !status.is_success() {
            return Err(anyhow!(
                "voice-model completion request failed with status {status}"
            ));
        }
        let payload: serde_json::Value = serde_json::from_str(&payload_text)
            .context("failed to parse voice-model completion body")?;
        let message = payload
            .get("choices")
            .and_then(|value| value.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .ok_or_else(|| anyhow!("voice-model completion did not contain a message"))?;
        let transcript = extract_chat_audio_transcript(message)
            .or_else(|_| extract_chat_message_text(message))?;
        let audio_base64 = message
            .get("audio")
            .and_then(|value| value.get("data"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("voice-model completion did not contain audio data"))?;
        let audio_bytes = BASE64
            .decode(audio_base64)
            .context("failed to decode voice-model audio payload")?;
        let (sample_rate, wav_samples) = decode_tts_audio("wav", Some("audio/wav"), &audio_bytes)?;

        Ok(VoiceResponseResult {
            text: transcript,
            pcm: resample_linear_mono(&wav_samples, sample_rate, TELEPHONY_RATE),
            usage: extract_usage(&payload),
        })
    }

    /// Starts the realtime transcription bridge in a background task.
    pub fn start_transcription_bridge(
        &self,
        sink: Arc<dyn TranscriptSink>,
    ) -> tokio::task::JoinHandle<()> {
        let config = self.config.clone();
        tokio::spawn(async move {
            if let Err(error) = run_transcription_session(config, sink).await {
                warn!(error = %error, "transcription bridge exited with error");
            }
        })
    }
}

/// Realtime websocket bridge for streaming telephony audio to OpenAI transcription.
pub struct TranscriptionBridge {
    sink: Arc<dyn TranscriptSink>,
    outbound_tx: mpsc::Sender<Vec<u8>>,
    last_error: Arc<RwLock<Option<String>>>,
}

impl TranscriptionBridge {
    /// Connects a new realtime transcription bridge.
    pub async fn connect(config: OpenAiConfig, sink: Arc<dyn TranscriptSink>) -> Result<Self> {
        let last_error = Arc::new(RwLock::new(None));
        let (outbound_tx, outbound_rx) = mpsc::channel(128);
        let last_error_clone = Arc::clone(&last_error);
        let sink_clone = Arc::clone(&sink);

        tokio::spawn(async move {
            if let Err(error) = run_transcription_loop(config, sink_clone, outbound_rx).await {
                *last_error_clone.write() = Some(error.to_string());
            }
        });

        Ok(Self {
            sink,
            outbound_tx,
            last_error,
        })
    }

    /// Queues a mu-law audio frame for transcription.
    pub async fn send_mulaw_frame(&self, samples: &[u8]) -> Result<()> {
        self.outbound_tx
            .send(samples.to_vec())
            .await
            .map_err(|_| anyhow!("transcription channel closed"))
    }

    /// Returns the last bridge error recorded by the background task.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.read().clone()
    }

    /// Records an error on the bridge and forwards it to the transcript sink.
    pub fn report_error(&self, error: impl Into<String>) {
        let message = error.into();
        *self.last_error.write() = Some(message.clone());
        self.sink.mark_error(message);
    }
}

async fn run_transcription_session(
    config: OpenAiConfig,
    sink: Arc<dyn TranscriptSink>,
) -> Result<()> {
    let (_tx, rx) = mpsc::channel(1);
    run_transcription_loop(config, sink, rx).await
}

async fn run_transcription_loop(
    config: OpenAiConfig,
    sink: Arc<dyn TranscriptSink>,
    mut outbound_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    let request = Request::builder()
        .uri(format!(
            "{}?model={}",
            config.realtime_url, config.transcription_model
        ))
        .header(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", config.api_key()))
                .context("invalid bearer token header")?,
        )
        .header("OpenAI-Beta", HeaderValue::from_static("realtime=v1"))
        .body(())
        .context("failed to build websocket request")?;

    let (stream, _) = connect_async(request)
        .await
        .context("failed to connect realtime transcription websocket")?;
    let (mut write, mut read) = stream.split();

    let session = json!({
        "type": "session.update",
        "session": {
            "input_audio_format": "g711_ulaw",
            "input_audio_transcription": {
                "model": config.transcription_model,
                "prompt": config.transcription_prompt,
                "language": config.transcription_language
            },
            "turn_detection": {
                "type": "server_vad"
            }
        }
    });
    write
        .send(Message::Text(session.to_string().into()))
        .await
        .context("failed to configure transcription session")?;

    loop {
        tokio::select! {
            maybe_audio = outbound_rx.recv() => {
                match maybe_audio {
                    Some(audio) => {
                        let payload = json!({
                            "type": "input_audio_buffer.append",
                            "audio": BASE64.encode(audio),
                        });
                        write
                            .send(Message::Text(payload.to_string().into()))
                            .await
                            .context("failed to send audio chunk to realtime api")?;
                    }
                    None => {
                        let commit = json!({ "type": "input_audio_buffer.commit" });
                        let _ = write.send(Message::Text(commit.to_string().into())).await;
                        let _ = write.close().await;
                        return Ok(());
                    }
                }
            }
            maybe_message = read.next() => {
                match maybe_message {
                    Some(Ok(Message::Text(payload))) => {
                        if let Some(event) = parse_transcript_event(&payload) {
                            sink.push_event(event);
                        }
                        if let Some(error) = parse_error_message(&payload) {
                            sink.mark_error(error);
                        }
                    }
                    Some(Ok(Message::Close(_))) => return Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error).context("realtime websocket read failed"),
                    None => return Ok(()),
                }
            }
        }
    }
}

fn parse_transcript_event(payload: &str) -> Option<TranscriptEvent> {
    let envelope: serde_json::Value = serde_json::from_str(payload).ok()?;
    let kind = envelope.get("type")?.as_str()?.to_string();
    let text = match kind.as_str() {
        "conversation.item.input_audio_transcription.delta" => envelope.get("delta")?.as_str()?,
        "conversation.item.input_audio_transcription.completed" => {
            envelope.get("transcript")?.as_str()?
        }
        _ => return None,
    };
    Some(TranscriptEvent {
        role: "caller".to_string(),
        kind,
        text: text.to_string(),
        at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok()?,
    })
}

fn parse_error_message(payload: &str) -> Option<String> {
    let envelope: serde_json::Value = serde_json::from_str(payload).ok()?;
    if envelope.get("type")?.as_str()? != "error" {
        return None;
    }
    Some(
        envelope
            .get("error")
            .and_then(|value| value.get("message"))
            .and_then(|value| value.as_str())
            .unwrap_or("unknown realtime api error")
            .to_string(),
    )
}

fn decode_tts_audio(
    response_format: &str,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<(u32, Vec<i16>)> {
    if response_format.eq_ignore_ascii_case("wav")
        || content_type.is_some_and(|value| value.contains("wav"))
        || bytes.starts_with(b"RIFF")
    {
        return decode_wav_mono_i16(bytes);
    }

    if let Ok(payload) = std::str::from_utf8(bytes)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(payload)
        && let Some(message) = json
            .get("error")
            .and_then(|value| value.get("message"))
            .and_then(|value| value.as_str())
    {
        return Err(anyhow!("TTS API returned an error payload: {message}"));
    }

    Err(anyhow!(
        "unsupported TTS payload format: response_format={}, content_type={}",
        response_format,
        content_type.unwrap_or("unknown")
    ))
}

fn extract_response_text(payload: &serde_json::Value) -> Result<String> {
    if let Some(text) = payload.get("output_text").and_then(|value| value.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(items) = payload.get("output").and_then(|value| value.as_array()) {
        let mut text = String::new();
        for item in items {
            if let Some(content) = item.get("content").and_then(|value| value.as_array()) {
                for block in content {
                    if let Some(value) = block.get("text").and_then(|value| value.as_str()) {
                        text.push_str(value);
                    }
                }
            }
        }
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    Err(anyhow!(
        "agent response payload did not contain output text"
    ))
}

fn extract_response_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("id")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn response_input(
    transcript: &[TranscriptEvent],
    previous_response_id: Option<&str>,
) -> Vec<serde_json::Value> {
    if previous_response_id.is_some()
        && let Some(latest_caller) = transcript
            .iter()
            .rev()
            .find(|event| event.role == "caller" && event.kind == "caller.transcript.completed")
    {
        return vec![json!({
            "role": "user",
            "content": latest_caller.text
        })];
    }

    transcript
        .iter()
        .filter(|event| {
            event.kind == "assistant.tts" || event.kind == "caller.transcript.completed"
        })
        .map(|event| {
            let role = if event.role == "caller" {
                "user"
            } else {
                "assistant"
            };
            json!({
                "role": role,
                "content": event.text
            })
        })
        .collect::<Vec<_>>()
}

fn audio_chat_messages(transcript: &[TranscriptEvent]) -> Vec<serde_json::Value> {
    transcript
        .iter()
        .filter(|event| {
            event.kind == "assistant.tts" || event.kind == "caller.transcript.completed"
        })
        .map(|event| {
            let role = if event.role == "caller" {
                "user"
            } else {
                "assistant"
            };
            json!({
                "role": role,
                "content": event.text,
            })
        })
        .collect()
}

fn extract_usage(payload: &serde_json::Value) -> TokenUsage {
    let usage = payload
        .get("usage")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    let input_details = usage
        .get("input_token_details")
        .or_else(|| usage.get("input_tokens_details"))
        .or_else(|| usage.get("prompt_tokens_details"));
    let output_details = usage
        .get("output_token_details")
        .or_else(|| usage.get("output_tokens_details"))
        .or_else(|| usage.get("completion_tokens_details"));

    let input_audio_tokens =
        value_u64(input_details.and_then(|details| details.get("audio_tokens")))
            .or_else(|| {
                value_u64(
                    usage
                        .get("input_audio_tokens")
                        .or_else(|| usage.get("audio_input_tokens")),
                )
            })
            .unwrap_or(0);
    let output_audio_tokens =
        value_u64(output_details.and_then(|details| details.get("audio_tokens")))
            .or_else(|| {
                value_u64(
                    usage
                        .get("output_audio_tokens")
                        .or_else(|| usage.get("audio_output_tokens")),
                )
            })
            .unwrap_or(0);
    let input_text_tokens = value_u64(input_details.and_then(|details| details.get("text_tokens")))
        .or_else(|| {
            value_u64(
                usage
                    .get("input_tokens")
                    .or_else(|| usage.get("prompt_tokens")),
            )
            .map(|tokens| tokens.saturating_sub(input_audio_tokens))
        })
        .unwrap_or(0);
    let cached_input_text_tokens =
        value_u64(input_details.and_then(|details| details.get("cached_tokens")))
            .or_else(|| {
                value_u64(
                    usage
                        .get("input_cached_tokens")
                        .or_else(|| usage.get("cached_input_tokens")),
                )
            })
            .unwrap_or(0);
    let output_text_tokens =
        value_u64(output_details.and_then(|details| details.get("text_tokens")))
            .or_else(|| {
                value_u64(
                    usage
                        .get("output_tokens")
                        .or_else(|| usage.get("completion_tokens")),
                )
                .map(|tokens| tokens.saturating_sub(output_audio_tokens))
            })
            .unwrap_or(0);

    TokenUsage {
        input_text_tokens,
        cached_input_text_tokens,
        output_text_tokens,
        input_audio_tokens,
        output_audio_tokens,
    }
}

fn extract_chat_audio_transcript(message: &serde_json::Value) -> Result<String> {
    let transcript = message
        .get("audio")
        .and_then(|value| value.get("transcript"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("chat completion message did not contain audio transcript"))?;
    Ok(transcript.to_string())
}

fn extract_chat_message_text(message: &serde_json::Value) -> Result<String> {
    if let Some(text) = message.get("content").and_then(|value| value.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(items) = message.get("content").and_then(|value| value.as_array()) {
        let mut text = String::new();
        for item in items {
            if let Some(value) = item.get("text").and_then(|value| value.as_str()) {
                text.push_str(value);
            }
        }
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    Err(anyhow!(
        "chat completion message did not contain assistant text"
    ))
}

fn value_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    value.and_then(|value| value.as_u64()).or_else(|| {
        value
            .and_then(|value| value.as_i64())
            .map(|value| value as u64)
    })
}

fn response_instructions(base: Option<&str>, context: &ConversationContext) -> String {
    let mut sections = Vec::new();
    let editable_fields = editable_field_names().join(", ");
    sections.push(
        base.unwrap_or(
            "You are a helpful voice agent on a phone call. Keep replies brief, natural, and conversational.",
        )
        .to_string(),
    );
    sections.push(
        format!(
            "You are {} on a phone call. Keep replies brief, natural, and helpful. It is currently {} for the caller.",
            context.assistant_name, context.time_of_day
        ),
    );
    sections.push(
        "Speak English unless the caller explicitly asks for another language in this call. Never comment on language detection; if audio is unclear, briefly ask them to repeat.".to_string(),
    );
    if let Some(summary) = context
        .known_caller
        .as_ref()
        .and_then(compact_caller_summary)
    {
        sections.push(format!("Known caller profile: {}.", summary));
    }
    if !context.missing_fields.is_empty() {
        sections.push(format!(
            "Missing profile fields: {}. Gather them naturally, at most one lightweight question at a time, only when helpful.",
            context.missing_fields.join(", ")
        ));
    }
    if context.phone_book_writable {
        sections.push(format!(
            "Only discuss or update the active caller record for caller ID {}. Editable fields: {}. If asked what can be updated, answer with that list plainly.",
            context.caller_id, editable_fields
        ));
    } else {
        sections.push(
            "Phone book writes are unavailable because the caller did not present usable caller ID. Answer generally, but do not claim to save or confirm profile details for this call."
                .to_string(),
        );
    }
    sections.push(
        "Never save, overwrite, or confirm details for another person's record. If they mention someone else, treat it as conversation only.".to_string(),
    );
    sections.push(
        "Do not rely on old notes to steer the conversation. Use saved profile details only when directly relevant.".to_string(),
    );
    sections.push(
        "Validation: first_name and last_name must be the caller's own name; company must be the caller's own workplace; infer timezone only from the caller's explicit location; set preferred_language only when explicitly requested.".to_string(),
    );
    sections.push(
        "Do not treat an email as saved until it has been spelled back and confirmed.".to_string(),
    );
    if let Some(email) = &context.pending_email_confirmation {
        sections.push(format!(
            "Pending email confirmation: {}. Confirm that exact spelling before treating it as saved.",
            email
        ));
    }
    sections.push(
        "If you know the caller's first name, use it naturally. If you still need their name, ask casually when it fits.".to_string(),
    );
    sections.push(
        "Return only JSON with keys `say` and `end_call`. `say` is exactly what will be spoken. Set `end_call` true only for a final closing farewell. If the caller asks to hang up after a simple final action, do that action first in the same reply, then close. Never set `end_call` true on clarifications or other non-final replies.".to_string(),
    );
    sections.join("\n")
}

fn compact_caller_summary(caller: &CallerRecord) -> Option<String> {
    let mut fields = Vec::new();
    if let Some(first_name) = caller.first_name.as_deref()
        && !first_name.trim().is_empty()
    {
        fields.push(format!("first_name={}", first_name.trim()));
    }
    if let Some(last_name) = caller.last_name.as_deref()
        && !last_name.trim().is_empty()
    {
        fields.push(format!("last_name={}", last_name.trim()));
    }
    if let Some(email) = caller.email.as_deref()
        && !email.trim().is_empty()
    {
        fields.push(format!("email={}", email.trim()));
    }
    if let Some(company) = caller.company.as_deref()
        && !company.trim().is_empty()
    {
        fields.push(format!("company={}", company.trim()));
    }
    if let Some(timezone) = caller.timezone.as_deref()
        && !timezone.trim().is_empty()
    {
        fields.push(format!("timezone={}", timezone.trim()));
    }
    if let Some(preferred_language) = caller.preferred_language.as_deref()
        && !preferred_language.trim().is_empty()
    {
        fields.push(format!("preferred_language={}", preferred_language.trim()));
    }
    if fields.is_empty() {
        None
    } else {
        Some(fields.join("; "))
    }
}

fn contact_extraction_instructions(caller: Option<&CallerRecord>) -> String {
    let existing = caller
        .map(|caller| prompt_safe_caller_json(caller).to_string())
        .unwrap_or_else(|| "null".to_string());
    format!(
        "You extract caller contact details from a phone transcript for the active caller only. Current known profile: {}. Return only a minified JSON object with keys first_name, last_name, email, company, timezone, preferred_language, notes. Use null for unknown scalar fields and [] for notes. Editable fields are {}. Only include facts explicitly learned about the active caller themself in this conversation. If the caller talks about another person, do not store that other person's details. If the caller explicitly mentions a city, state, region, or country they are from or located in, infer the best matching IANA timezone like Australia/Sydney or America/Chicago and place it in timezone. If no explicit location was given, use null. Set preferred_language only when the caller explicitly states a contact or conversation language preference, asks to continue in a specific language, or clearly says they prefer a language. Do not infer preferred_language from accent, detected speech language, caller history, or general language guessing. Only return email when the caller clearly stated their own email address. Notes should be short factual fragments, not prose, and should only contain low-risk caller-specific preferences or facts. Do not store language guesses, language history, or translation preferences in notes.",
        existing,
        editable_field_names().join(", ")
    )
}

fn prompt_safe_caller_json(caller: &CallerRecord) -> serde_json::Value {
    let filtered_notes = caller
        .notes
        .iter()
        .filter(|note| !note_mentions_language(note))
        .cloned()
        .collect::<Vec<_>>();
    let note_count = filtered_notes.len();
    let filtered_notes = filtered_notes
        .into_iter()
        .skip(note_count.saturating_sub(3))
        .collect::<Vec<_>>();
    json!({
        "caller_id": caller.caller_id,
        "first_seen_at": caller.first_seen_at,
        "last_seen_at": caller.last_seen_at,
        "call_count": caller.call_count,
        "first_name": caller.first_name,
        "last_name": caller.last_name,
        "email": caller.email,
        "company": caller.company,
        "timezone": caller.timezone,
        "preferred_language": caller.preferred_language,
        "notes": filtered_notes,
    })
}

fn note_mentions_language(note: &str) -> bool {
    let normalized = note.to_ascii_lowercase();
    normalized.contains(" language")
        || normalized.starts_with("language ")
        || normalized.contains("speaks ")
        || normalized.contains("spoke ")
        || normalized.contains("used ")
}

fn extract_turn_plan(payload: &serde_json::Value) -> Result<TurnPlan> {
    let text = extract_response_text(payload)?;
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```");
    let trimmed = trimmed.trim_end_matches("```").trim();
    let plan: TurnPlan =
        serde_json::from_str(trimmed).context("failed to parse agent turn plan JSON")?;
    if plan.say.trim().is_empty() {
        return Err(anyhow!("agent turn plan did not contain spoken text"));
    }
    Ok(TurnPlan {
        say: plan.say.trim().to_string(),
        end_call: plan.end_call,
    })
}

fn sanitize_turn_plan(plan: TurnPlan, transcript: &[TranscriptEvent]) -> TurnPlan {
    let latest_caller_text = latest_caller_text(transcript);
    if looks_like_language_comment(&plan.say) {
        if caller_explicitly_discussed_language(latest_caller_text) {
            return plan;
        }

        return TurnPlan {
            say: "Sorry, I didn't quite catch that. Could you say that again?".to_string(),
            end_call: false,
        };
    }

    let caller_requested_end_call = caller_requested_end_call(latest_caller_text);
    let say = plan.say.trim();
    if caller_requested_end_call {
        if let Some(limit) = requested_count_before_hangup(latest_caller_text)
            && !response_mentions_requested_count(say, limit)
        {
            return TurnPlan {
                say: counted_final_farewell(limit),
                end_call: true,
            };
        }
        return TurnPlan {
            say: finalize_end_call_response(say),
            end_call: true,
        };
    }
    if plan.end_call && !looks_like_final_farewell(say) {
        return TurnPlan {
            say: say.to_string(),
            end_call: false,
        };
    }

    plan
}

fn latest_caller_text(transcript: &[TranscriptEvent]) -> &str {
    transcript
        .iter()
        .rev()
        .find(|event| event.role == "caller" && event.kind == "caller.transcript.completed")
        .map(|event| event.text.as_str())
        .unwrap_or_default()
}

fn looks_like_language_comment(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("switching languages")
        || normalized.contains("another language")
        || normalized.contains("different language")
        || normalized.contains("what language")
        || normalized.contains("which language")
        || normalized.contains("language you prefer")
}

fn caller_explicitly_discussed_language(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("language")
        || normalized.contains("english")
        || normalized.contains("korean")
        || normalized.contains("japanese")
        || normalized.contains("mandarin")
        || normalized.contains("cantonese")
        || normalized.contains("spanish")
        || normalized.contains("french")
        || normalized.contains("german")
        || normalized.contains("arabic")
        || normalized.contains("italian")
        || normalized.contains("portuguese")
        || normalized.contains("hindi")
        || normalized.contains("vietnamese")
        || normalized.contains("thai")
        || normalized.contains("speak ")
        || normalized.contains("translation")
}

fn caller_requested_end_call(text: &str) -> bool {
    let normalized = normalize_match_text(text);
    [
        "goodbye",
        "good bye",
        "bye",
        "bye bye",
        "see you",
        "see ya",
        "catch you later",
        "talk to you later",
        "that s all",
        "that is all",
        "nothing else",
        "hang up",
        "hanging up",
        "hangup",
        "disconnect",
        "end the call",
        "drop the call",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

fn looks_like_final_farewell(text: &str) -> bool {
    let normalized = normalize_match_text(text);
    [
        "goodbye",
        "good bye",
        "bye",
        "see you",
        "see ya",
        "take care",
        "have a good",
        "have a great",
        "talk soon",
        "catch you later",
        "call back",
        "thanks for calling",
        "no worries bye",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

fn finalize_end_call_response(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || looks_like_clarification_request(trimmed) {
        return "Okay, no worries. See you later.".to_string();
    }
    if looks_like_final_farewell(trimmed) {
        return trimmed.to_string();
    }
    let suffix = " See you later.";
    if trimmed.ends_with(['.', '!', '?']) {
        format!("{trimmed}{suffix}")
    } else {
        format!("{trimmed}.{suffix}")
    }
}

fn looks_like_clarification_request(text: &str) -> bool {
    let normalized = normalize_match_text(text);
    [
        "could you say that again",
        "can you say that again",
        "could you repeat that",
        "can you repeat that",
        "i didn t catch that",
        "i did not catch that",
        "pardon",
        "sorry",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

fn normalize_match_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_was_space = true;
    for ch in text.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            ' '
        };
        if mapped == ' ' {
            if !last_was_space {
                normalized.push(' ');
            }
            last_was_space = true;
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }
    normalized.trim().to_string()
}

fn requested_count_before_hangup(text: &str) -> Option<u8> {
    let normalized = normalize_match_text(text);
    if !(normalized.contains("count to") && caller_requested_end_call(&normalized)) {
        return None;
    }
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for window in tokens.windows(3) {
        if window[0] == "count"
            && window[1] == "to"
            && let Some(limit) = parse_small_count(window[2])
        {
            return Some(limit);
        }
    }
    None
}

fn parse_small_count(token: &str) -> Option<u8> {
    match token {
        "1" | "one" => Some(1),
        "2" | "two" => Some(2),
        "3" | "three" => Some(3),
        "4" | "four" => Some(4),
        "5" | "five" => Some(5),
        "6" | "six" => Some(6),
        "7" | "seven" => Some(7),
        "8" | "eight" => Some(8),
        "9" | "nine" => Some(9),
        "10" | "ten" => Some(10),
        _ => None,
    }
}

fn response_mentions_requested_count(text: &str, limit: u8) -> bool {
    let normalized = normalize_match_text(text);
    let spoken = count_tokens(limit).join(" ");
    let numeric = (1..=limit)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    normalized.contains(&spoken) || normalized.contains(&numeric)
}

fn counted_final_farewell(limit: u8) -> String {
    format!("{}, see you later.", count_tokens(limit).join(", "))
}

fn count_tokens(limit: u8) -> Vec<&'static str> {
    let mut tokens = Vec::new();
    for value in 1..=limit {
        tokens.push(match value {
            1 => "one",
            2 => "two",
            3 => "three",
            4 => "four",
            5 => "five",
            6 => "six",
            7 => "seven",
            8 => "eight",
            9 => "nine",
            10 => "ten",
            _ => break,
        });
    }
    tokens
}

fn parse_caller_update(text: &str) -> Result<CallerUpdate> {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```");
    let trimmed = trimmed.trim_end_matches("```").trim();
    serde_json::from_str(trimmed).context("failed to parse caller update JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::TELEPHONY_FRAME_SAMPLES;

    #[test]
    fn parse_completed_transcript_event() {
        let payload = r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"hello world"}"#;
        let event = parse_transcript_event(payload).expect("event");
        assert_eq!(
            event.kind,
            "conversation.item.input_audio_transcription.completed"
        );
        assert_eq!(event.text, "hello world");
    }

    #[test]
    fn parse_delta_transcript_event() {
        let payload =
            r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"hel"}"#;
        let event = parse_transcript_event(payload).expect("event");
        assert_eq!(event.text, "hel");
    }

    #[test]
    fn parse_error_event() {
        let payload = r#"{"type":"error","error":{"message":"bad audio"}}"#;
        assert_eq!(parse_error_message(payload).as_deref(), Some("bad audio"));
    }

    #[test]
    fn telephony_frame_constant_matches_g711_interval() {
        assert_eq!(TELEPHONY_FRAME_SAMPLES, 160);
    }

    #[test]
    fn decode_tts_audio_reports_json_error_payload() {
        let error = decode_tts_audio(
            "pcm",
            Some("application/json"),
            br#"{"error":{"message":"bad api key"}}"#,
        )
        .expect_err("json payload should fail");

        assert!(error.to_string().contains("bad api key"));
    }

    #[test]
    fn extract_response_text_prefers_output_text() {
        let payload = json!({
            "output_text": "Hello there"
        });
        let text = extract_response_text(&payload).expect("response text");
        assert_eq!(text, "Hello there");
    }

    #[test]
    fn extract_response_text_reads_output_blocks() {
        let payload = json!({
            "output": [
                {
                    "content": [
                        {"type": "output_text", "text": "Hello"},
                        {"type": "output_text", "text": " there"}
                    ]
                }
            ]
        });
        let text = extract_response_text(&payload).expect("response text");
        assert_eq!(text, "Hello there");
    }

    #[test]
    fn extract_chat_audio_transcript_reads_message_audio() {
        let message = json!({
            "audio": {
                "transcript": "Sure, here you go."
            }
        });

        let transcript = extract_chat_audio_transcript(&message).expect("audio transcript");
        assert_eq!(transcript, "Sure, here you go.");
    }

    #[test]
    fn audio_chat_messages_replay_caller_and_assistant_turns() {
        let transcript = vec![
            TranscriptEvent {
                role: "caller".to_string(),
                kind: "caller.transcript.completed".to_string(),
                text: "Hello".to_string(),
                at: "2026-03-18T00:00:00Z".to_string(),
            },
            TranscriptEvent {
                role: "assistant".to_string(),
                kind: "assistant.tts".to_string(),
                text: "Hi there".to_string(),
                at: "2026-03-18T00:00:01Z".to_string(),
            },
        ];

        let messages = audio_chat_messages(&transcript);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "Hi there");
    }

    #[test]
    fn prompt_safe_caller_json_filters_language_notes() {
        let caller = CallerRecord {
            caller_id: "6140000".to_string(),
            first_seen_at: "2026-03-15T00:00:00Z".to_string(),
            last_seen_at: "2026-03-15T00:00:01Z".to_string(),
            call_count: 1,
            disabled: false,
            system_entry: false,
            first_name: Some("David".to_string()),
            last_name: None,
            email: None,
            company: None,
            timezone: Some("Australia/Sydney".to_string()),
            preferred_language: Some("English".to_string()),
            notes: vec![
                "caller used Korean language".to_string(),
                "looking to buy a toaster".to_string(),
            ],
        };

        let safe = prompt_safe_caller_json(&caller);
        let notes = safe
            .get("notes")
            .and_then(|value| value.as_array())
            .expect("notes array");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].as_str(), Some("looking to buy a toaster"));
        assert_eq!(
            safe.get("preferred_language")
                .and_then(|value| value.as_str()),
            Some("English")
        );
    }

    #[test]
    fn parse_caller_update_reads_preferred_language() {
        let update = parse_caller_update(
            r#"{"first_name":"Dave","last_name":null,"email":null,"company":null,"timezone":"Australia/Sydney","preferred_language":"English","notes":[]}"#,
        )
        .expect("caller update");

        assert_eq!(update.first_name.as_deref(), Some("Dave"));
        assert_eq!(update.preferred_language.as_deref(), Some("English"));
    }

    #[test]
    fn extract_turn_plan_parses_json_payload() {
        let payload = json!({
            "output_text": "{\"say\":\"Okay, see you later.\",\"end_call\":true}"
        });

        let plan = extract_turn_plan(&payload).expect("turn plan");
        assert_eq!(plan.say, "Okay, see you later.");
        assert!(plan.end_call);
    }

    #[test]
    fn response_input_uses_latest_caller_turn_when_chained() {
        let transcript = vec![
            TranscriptEvent {
                role: "assistant".to_string(),
                kind: "assistant.tts".to_string(),
                text: "How can I help?".to_string(),
                at: "2026-03-16T00:00:00Z".to_string(),
            },
            TranscriptEvent {
                role: "caller".to_string(),
                kind: "caller.transcript.completed".to_string(),
                text: "Update my email.".to_string(),
                at: "2026-03-16T00:00:01Z".to_string(),
            },
        ];

        let input = response_input(&transcript, Some("resp_123"));
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "Update my email.");
    }

    #[test]
    fn extract_response_id_reads_top_level_id() {
        let payload = json!({
            "id": "resp_123",
            "output_text": "{\"say\":\"Hello\",\"end_call\":false}"
        });

        assert_eq!(extract_response_id(&payload).as_deref(), Some("resp_123"));
    }

    #[test]
    fn sanitize_turn_plan_rewrites_language_switching_prompt() {
        let transcript = vec![TranscriptEvent {
            role: "caller".to_string(),
            kind: "caller.transcript.completed".to_string(),
            text: "Nonostante".to_string(),
            at: "2026-03-15T00:00:00Z".to_string(),
        }];

        let sanitized = sanitize_turn_plan(
            TurnPlan {
                say: "It seems like we might be switching languages. Could you let me know how I can assist you today?".to_string(),
                end_call: false,
            },
            &transcript,
        );

        assert_eq!(
            sanitized.say,
            "Sorry, I didn't quite catch that. Could you say that again?"
        );
        assert!(!sanitized.end_call);
    }

    #[test]
    fn sanitize_turn_plan_keeps_explicit_language_requests() {
        let transcript = vec![TranscriptEvent {
            role: "caller".to_string(),
            kind: "caller.transcript.completed".to_string(),
            text: "Can you speak Korean?".to_string(),
            at: "2026-03-15T00:00:00Z".to_string(),
        }];

        let original = TurnPlan {
            say: "Which language would you like to use?".to_string(),
            end_call: false,
        };
        let sanitized = sanitize_turn_plan(original.clone(), &transcript);

        assert_eq!(sanitized.say, original.say);
        assert_eq!(sanitized.end_call, original.end_call);
    }

    #[test]
    fn sanitize_turn_plan_promotes_explicit_hangup_request_to_final_closing() {
        let transcript = vec![TranscriptEvent {
            role: "caller".to_string(),
            kind: "caller.transcript.completed".to_string(),
            text: "Please count to three and hang up.".to_string(),
            at: "2026-03-16T00:00:00Z".to_string(),
        }];

        let sanitized = sanitize_turn_plan(
            TurnPlan {
                say: "One, two, three.".to_string(),
                end_call: false,
            },
            &transcript,
        );

        assert_eq!(sanitized.say, "One, two, three. See you later.");
        assert!(sanitized.end_call);
    }

    #[test]
    fn sanitize_turn_plan_fulfills_count_then_hangup_request_when_model_skips_count() {
        let transcript = vec![TranscriptEvent {
            role: "caller".to_string(),
            kind: "caller.transcript.completed".to_string(),
            text: "Count to 3 and hang up.".to_string(),
            at: "2026-03-16T00:00:00Z".to_string(),
        }];

        let sanitized = sanitize_turn_plan(
            TurnPlan {
                say: "Okay, no worries. See you later.".to_string(),
                end_call: true,
            },
            &transcript,
        );

        assert_eq!(sanitized.say, "one, two, three, see you later.");
        assert!(sanitized.end_call);
    }

    #[test]
    fn sanitize_turn_plan_clears_end_call_for_non_closing_reply() {
        let transcript = vec![TranscriptEvent {
            role: "caller".to_string(),
            kind: "caller.transcript.completed".to_string(),
            text: "Tell me a joke.".to_string(),
            at: "2026-03-16T00:00:00Z".to_string(),
        }];

        let sanitized = sanitize_turn_plan(
            TurnPlan {
                say: "I didn't catch that, could you repeat it?".to_string(),
                end_call: true,
            },
            &transcript,
        );

        assert_eq!(sanitized.say, "I didn't catch that, could you repeat it?");
        assert!(!sanitized.end_call);
    }

    #[test]
    fn compact_caller_summary_omits_notes_and_empty_fields() {
        let caller = CallerRecord {
            caller_id: "61400000000".to_string(),
            first_seen_at: "2026-03-16T00:00:00Z".to_string(),
            last_seen_at: "2026-03-16T00:00:00Z".to_string(),
            call_count: 3,
            system_entry: false,
            first_name: Some("Dave".to_string()),
            last_name: None,
            email: None,
            company: Some("Example".to_string()),
            timezone: Some("Australia/Sydney".to_string()),
            preferred_language: None,
            notes: vec!["likes rugby".to_string()],
            disabled: false,
        };

        let summary = compact_caller_summary(&caller).expect("summary");
        assert_eq!(
            summary,
            "first_name=Dave; company=Example; timezone=Australia/Sydney"
        );
    }

    #[test]
    fn extract_usage_reads_audio_and_cached_tokens() {
        let payload = json!({
            "usage": {
                "input_tokens": 120,
                "output_tokens": 45,
                "input_tokens_details": {
                    "cached_tokens": 20,
                    "audio_tokens": 30
                },
                "output_tokens_details": {
                    "audio_tokens": 5
                }
            }
        });

        let usage = extract_usage(&payload);
        assert_eq!(usage.input_text_tokens, 90);
        assert_eq!(usage.cached_input_text_tokens, 20);
        assert_eq!(usage.output_text_tokens, 40);
        assert_eq!(usage.input_audio_tokens, 30);
        assert_eq!(usage.output_audio_tokens, 5);
    }
}
