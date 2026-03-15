//! OpenAI client integration for STT, TTS, and response generation.

use std::sync::Arc;

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
use crate::config::OpenAiConfig;
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

        debug!(
            endpoint = %self.config.audio_api_url,
            request_body = %body,
            "sending OpenAI TTS request"
        );
        let response = self
            .client
            .post(&self.config.audio_api_url)
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
                time_of_day: "day".to_string(),
                known_caller: None,
                missing_fields: Vec::new(),
                pending_email_confirmation: None,
            },
        )
        .await?
        .text)
    }

    /// Generates a structured assistant response with full conversation context.
    pub async fn generate_response_with_context(
        &self,
        transcript: &[TranscriptEvent],
        context: &ConversationContext,
    ) -> Result<ResponseResult> {
        let input = transcript
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
            .collect::<Vec<_>>();

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
        body["instructions"] = json!(response_instructions(
            self.config.response_instructions.as_deref(),
            context,
        ));

        debug!(
            endpoint = %self.config.responses_api_url,
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

fn extract_usage(payload: &serde_json::Value) -> TokenUsage {
    let usage = payload
        .get("usage")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    let input_details = usage
        .get("input_token_details")
        .or_else(|| usage.get("input_tokens_details"));
    let output_details = usage
        .get("output_token_details")
        .or_else(|| usage.get("output_tokens_details"));

    let input_audio_tokens = value_u64(input_details.and_then(|details| details.get("audio_tokens")))
        .or_else(|| {
            value_u64(
                usage.get("input_audio_tokens")
                    .or_else(|| usage.get("audio_input_tokens")),
            )
        })
        .unwrap_or(0);
    let output_audio_tokens =
        value_u64(output_details.and_then(|details| details.get("audio_tokens")))
            .or_else(|| {
                value_u64(
                    usage.get("output_audio_tokens")
                        .or_else(|| usage.get("audio_output_tokens")),
                )
            })
            .unwrap_or(0);
    let input_text_tokens = value_u64(input_details.and_then(|details| details.get("text_tokens")))
        .or_else(|| {
            value_u64(usage.get("input_tokens"))
                .map(|tokens| tokens.saturating_sub(input_audio_tokens))
        })
        .unwrap_or(0);
    let cached_input_text_tokens =
        value_u64(input_details.and_then(|details| details.get("cached_tokens")))
            .or_else(|| {
                value_u64(
                    usage.get("input_cached_tokens")
                        .or_else(|| usage.get("cached_input_tokens")),
                )
            })
            .unwrap_or(0);
    let output_text_tokens =
        value_u64(output_details.and_then(|details| details.get("text_tokens")))
            .or_else(|| {
                value_u64(usage.get("output_tokens"))
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

fn value_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    value
        .and_then(|value| value.as_u64())
        .or_else(|| value.and_then(|value| value.as_i64()).map(|value| value as u64))
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
    sections.push(format!(
        "You are answering phone calls as {}.",
        context.assistant_name
    ));
    sections.push(format!(
        "Current time of day for the caller greeting: {}.",
        context.time_of_day
    ));
    sections.push(
        "Speak English by default. Do not switch languages based on caller history or notes alone. Only use another language if the caller speaks that language in the current call or explicitly asks you to.".to_string(),
    );
    sections.push(
        "Never say that the caller seems to be switching languages and never comment on language detection. If the latest caller utterance is unclear, garbled, or not actionable, briefly ask them to repeat or clarify what they need in English.".to_string(),
    );
    sections.push(format!("Caller ID: {}.", context.caller_id));
    if let Some(caller) = &context.known_caller {
        sections.push(format!(
            "Known caller profile: {}",
            prompt_safe_caller_json(caller)
        ));
    } else {
        sections.push("This caller is not known yet.".to_string());
    }
    if !context.missing_fields.is_empty() {
        sections.push(format!(
            "Missing caller fields: {}. Try to gather these naturally over time without making the conversation awkward. Prefer one lightweight question at a time.",
            context.missing_fields.join(", ")
        ));
    }
    sections.push(format!(
        "Phone book tool: you may only discuss or update the active caller record for caller ID {}. Available editable fields are: {}. If the caller asks what fields are available, answer with that list plainly.",
        context.caller_id, editable_fields
    ));
    sections.push(
        "Never let the caller set, overwrite, or confirm contact details for another person's record. If they mention someone else, treat that as conversation only and do not store it.".to_string(),
    );
    sections.push(
        "Caller notes are low-priority memory only. Use them only when directly relevant, and do not steer the conversation back to old notes unless the caller brings them up.".to_string(),
    );
    sections.push(
        "Validation rules: first_name and last_name must be clearly given as the caller's own name; company must be clearly described as the caller's own company or workplace; timezone may be inferred only from the caller's own explicit location; preferred_language may be stored only when the caller explicitly says they prefer or want a language.".to_string(),
    );
    sections.push(
        "Do not store an email address until it has been repeated back and confirmed. If the caller asks to update their email or gives an email, ask them to spell it carefully and confirm it before treating it as saved.".to_string(),
    );
    if let Some(email) = &context.pending_email_confirmation {
        sections.push(format!(
            "Pending email confirmation: {}. Before treating it as saved, ask the caller to confirm whether that exact email is correct unless they have already just confirmed it.",
            email
        ));
    }
    sections.push(
        "If the caller is known by first name, greet them naturally by that name. If you know the first name but not the last name, ask casually for the last name when it fits. If you do not know their name yet, ask naturally who is calling when appropriate. Do not interrogate them.".to_string(),
    );
    sections.push(
        "Return only a JSON object with keys `say` and `end_call`. `say` must contain exactly what you want spoken to the caller. Set `end_call` to true only when your spoken reply is a final closing farewell and the phone should hang up immediately after playback. If the caller says goodbye but it is still appropriate to ask whether they need anything else, keep `end_call` false for that turn.".to_string(),
    );
    sections.join("\n")
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
    if !looks_like_language_comment(&plan.say) {
        return plan;
    }

    let latest_caller_text = transcript
        .iter()
        .rev()
        .find(|event| event.role == "caller" && event.kind == "caller.transcript.completed")
        .map(|event| event.text.as_str())
        .unwrap_or_default();
    if caller_explicitly_discussed_language(latest_caller_text) {
        return plan;
    }

    TurnPlan {
        say: "Sorry, I didn't quite catch that. Could you say that again?".to_string(),
        end_call: false,
    }
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
    fn prompt_safe_caller_json_filters_language_notes() {
        let caller = CallerRecord {
            caller_id: "6140000".to_string(),
            first_seen_at: "2026-03-15T00:00:00Z".to_string(),
            last_seen_at: "2026-03-15T00:00:01Z".to_string(),
            call_count: 1,
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
            safe.get("preferred_language").and_then(|value| value.as_str()),
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
