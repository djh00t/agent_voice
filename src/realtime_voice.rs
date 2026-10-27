//! Streaming Realtime voice adapters for OpenAI and xAI.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use http::{HeaderValue, Request};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::{MissedTickBehavior, interval};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::warn;

use crate::accounting::TokenUsage;
use crate::openai::extract_usage;

const PCMU_BYTES_PER_FRAME: usize = 160;
const OUTPUT_FRAME_INTERVAL: Duration = Duration::from_millis(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_QUEUED_OUTPUT_BYTES: usize = 160_000;
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(5);
const PCMU_SILENCE: u8 = 0xFF;

/// Sends provider PCMU audio to the call media path.
pub trait RealtimeAudioSink: Send + Sync {
    /// Sends one PCMU frame. The marker bit starts a new provider talkspurt.
    fn send_audio(&self, audio: &[u8], marker: bool) -> Result<()>;
    /// Drops queued output for barge-in.
    fn flush(&self);
}

/// Provider connection settings. Debug output never includes the credential.
#[derive(Clone)]
pub struct RealtimeConnectionConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub voice: String,
    pub instructions: Option<String>,
    pub transcription_model: Option<String>,
    pub language_hint: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl fmt::Debug for RealtimeConnectionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RealtimeConnectionConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("voice", &self.voice)
            .field("instructions", &self.instructions)
            .field("transcription_model", &self.transcription_model)
            .field("language_hint", &self.language_hint)
            .field("reasoning_effort", &self.reasoning_effort)
            .finish()
    }
}

/// Minimal OpenAI/xAI protocol adapter.
#[derive(Clone, Debug)]
pub enum RealtimeAdapter {
    OpenAi(RealtimeConnectionConfig),
    Xai(RealtimeConnectionConfig),
}

impl RealtimeAdapter {
    fn config(&self) -> &RealtimeConnectionConfig {
        match self {
            Self::OpenAi(config) | Self::Xai(config) => config,
        }
    }

    /// Returns the provider label used by the control API and accounting.
    pub fn name(&self) -> &'static str {
        match self {
            Self::OpenAi(_) => "openai",
            Self::Xai(_) => "xai",
        }
    }

    /// Returns the provider endpoint without credentials.
    pub fn endpoint(&self) -> &str {
        &self.config().endpoint
    }

    /// Returns the selected realtime model.
    pub fn model(&self) -> &str {
        &self.config().model
    }

    fn websocket_request(&self) -> Result<Request<()>> {
        let config = self.config();
        let mut url = url::Url::parse(&config.endpoint)
            .with_context(|| format!("invalid {} realtime URL", self.name()))?;
        if !matches!(url.scheme(), "ws" | "wss")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            bail!("{} realtime URL must use ws or wss", self.name());
        }
        url.query_pairs_mut().append_pair("model", &config.model);
        let authorization = HeaderValue::from_str(&format!("Bearer {}", config.api_key))
            .context("invalid realtime bearer token")?;
        let mut request = url
            .as_str()
            .into_client_request()
            .context("failed to build realtime websocket request")?;
        request.headers_mut().insert("Authorization", authorization);
        Ok(request)
    }

    fn session_update(&self) -> Value {
        let config = self.config();
        match self {
            Self::OpenAi(_) => {
                let mut input = json!({
                    "format": {"type": "audio/pcmu"},
                    "turn_detection": {"type": "server_vad"},
                    "transcription": {"model": "gpt-4o-transcribe"}
                });
                if let Some(model) = config.transcription_model.as_deref()
                    && !model.trim().is_empty()
                {
                    input["transcription"]["model"] = json!(model);
                }
                let mut session = json!({
                    "type": "realtime",
                    "audio": {
                        "input": input,
                        "output": {
                            "format": {"type": "audio/pcmu"},
                            "voice": config.voice
                        }
                    }
                });
                if let Some(instructions) = config
                    .instructions
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    session["instructions"] = json!(instructions);
                }
                json!({"type": "session.update", "session": session})
            }
            Self::Xai(_) => {
                let mut transcription = json!({
                    "model": config.transcription_model.as_deref().unwrap_or("grok-transcribe")
                });
                if let Some(language) = config.language_hint.as_deref()
                    && !language.trim().is_empty()
                {
                    transcription["language_hint"] = json!(language);
                }
                let mut session = json!({
                    "reasoning": {
                        "effort": config.reasoning_effort.as_deref().unwrap_or("none")
                    },
                    "audio": {
                        "input": {
                            "format": {"type": "audio/pcmu"},
                            "turn_detection": {"type": "server_vad"},
                            "transcription": transcription
                        },
                        "output": {
                            "format": {"type": "audio/pcmu"},
                            "voice": config.voice
                        }
                    }
                });
                if let Some(instructions) = config
                    .instructions
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    session["instructions"] = json!(instructions);
                }
                json!({"type": "session.update", "session": session})
            }
        }
    }

    fn speak_events(
        &self,
        text: &str,
        voice: Option<&str>,
        instructions: Option<&str>,
    ) -> Vec<Value> {
        match self {
            Self::OpenAi(_) => {
                let instruction = match instructions.map(str::trim).filter(|text| !text.is_empty())
                {
                    Some(instructions) => format!(
                        "{instructions}\nSay the following text exactly, without adding anything: {text}"
                    ),
                    None => {
                        format!("Say the following text exactly, without adding anything: {text}")
                    }
                };
                let mut response = json!({"instructions": instruction});
                if let Some(voice) = voice.map(str::trim).filter(|voice| !voice.is_empty()) {
                    response["voice"] = json!(voice);
                }
                vec![
                    json!({
                        "type": "conversation.item.create",
                        "item": {
                            "type": "message",
                            "role": "user",
                            "content": [{"type": "input_text", "text": instruction}]
                        }
                    }),
                    json!({"type": "response.create", "response": response}),
                ]
            }
            Self::Xai(_) => vec![json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "force_message",
                    "role": "assistant",
                    "interruptible": false,
                    "content": [{"type": "output_text", "text": text}]
                }
            })],
        }
    }
}

/// Normalized realtime events used by the SIP bridge.
#[derive(Clone)]
pub enum RealtimeEvent {
    AudioDelta {
        audio: Vec<u8>,
        item_id: Option<String>,
    },
    InputTranscript {
        text: String,
        final_text: bool,
    },
    OutputTranscript {
        text: String,
        final_text: bool,
    },
    SpeechStarted,
    ResponseDone {
        usage: Option<TokenUsage>,
    },
    Error {
        error_type: Option<String>,
        code: Option<String>,
    },
}

impl fmt::Debug for RealtimeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::AudioDelta { audio, .. } => format!("AudioDelta({} bytes)", audio.len()),
            Self::InputTranscript { final_text, .. } => {
                format!("InputTranscript(final={final_text})")
            }
            Self::OutputTranscript { final_text, .. } => {
                format!("OutputTranscript(final={final_text})")
            }
            Self::SpeechStarted => "SpeechStarted".to_string(),
            Self::ResponseDone { usage } => format!("ResponseDone(usage={})", usage.is_some()),
            Self::Error { .. } => "Error".to_string(),
        };
        formatter.write_str(&kind)
    }
}

struct EventMapper {
    adapter: RealtimeAdapter,
    input_pending: String,
    output_pending: String,
    response_active: bool,
    response_item_id: Option<String>,
}

impl EventMapper {
    fn new(adapter: RealtimeAdapter) -> Self {
        Self {
            adapter,
            input_pending: String::new(),
            output_pending: String::new(),
            response_active: false,
            response_item_id: None,
        }
    }

    fn map(&mut self, event: &Value) -> Vec<RealtimeEvent> {
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            return Vec::new();
        };
        let mut mapped = Vec::new();
        match kind {
            "response.created" => {
                self.response_active = true;
                self.response_item_id = None;
                if matches!(self.adapter, RealtimeAdapter::Xai(_))
                    && let Some(event) = self.finalize_input()
                {
                    mapped.push(event);
                }
            }
            "response.output_item.added" => {
                self.response_item_id = event
                    .get("item")
                    .and_then(|item| item.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
            }
            "conversation.item.input_audio_transcription.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.input_pending.push_str(delta);
                    mapped.push(RealtimeEvent::InputTranscript {
                        text: self.input_pending.clone(),
                        final_text: false,
                    });
                }
            }
            "conversation.item.input_audio_transcription.updated" => {
                if let Some(text) = event
                    .get("transcript")
                    .or_else(|| event.get("text"))
                    .and_then(Value::as_str)
                {
                    self.input_pending.clear();
                    self.input_pending.push_str(text);
                    mapped.push(RealtimeEvent::InputTranscript {
                        text: self.input_pending.clone(),
                        final_text: false,
                    });
                }
            }
            "conversation.item.input_audio_transcription.completed" => {
                if let Some(text) = event.get("transcript").and_then(Value::as_str) {
                    self.input_pending.clear();
                    mapped.push(RealtimeEvent::InputTranscript {
                        text: text.to_string(),
                        final_text: true,
                    });
                }
                if matches!(self.adapter, RealtimeAdapter::Xai(_)) {
                    self.input_pending.clear();
                }
            }
            "response.output_audio.delta" => {
                self.ready_for_output(&mut mapped);
                let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                    return mapped;
                };
                match BASE64.decode(delta) {
                    Ok(audio) => {
                        let item_id = event
                            .get("item_id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                            .or_else(|| self.response_item_id.clone());
                        mapped.push(RealtimeEvent::AudioDelta { audio, item_id });
                    }
                    Err(_) => mapped.push(RealtimeEvent::Error {
                        error_type: Some("invalid_audio".to_string()),
                        code: None,
                    }),
                }
            }
            "response.output_audio_transcript.delta" => {
                self.ready_for_output(&mut mapped);
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.output_pending.push_str(delta);
                    mapped.push(RealtimeEvent::OutputTranscript {
                        text: self.output_pending.clone(),
                        final_text: false,
                    });
                }
            }
            "response.output_audio_transcript.done" => {
                if let Some(text) = event.get("transcript").and_then(Value::as_str) {
                    self.output_pending.clear();
                    mapped.push(RealtimeEvent::OutputTranscript {
                        text: text.to_string(),
                        final_text: true,
                    });
                }
            }
            "input_audio_buffer.speech_started" => {
                mapped.push(RealtimeEvent::SpeechStarted);
            }
            "response.done" => {
                if let Some(event) = self.finalize_input() {
                    mapped.insert(0, event);
                }
                if !self.output_pending.is_empty() {
                    mapped.push(RealtimeEvent::OutputTranscript {
                        text: std::mem::take(&mut self.output_pending),
                        final_text: true,
                    });
                }
                self.response_active = false;
                self.response_item_id = None;
                mapped.push(RealtimeEvent::ResponseDone {
                    usage: event
                        .get("response")
                        .and_then(|response| response.get("usage"))
                        .map(|usage| extract_usage(&json!({"usage": usage}))),
                });
            }
            "error" => {
                let error = event.get("error").unwrap_or(event);
                mapped.push(RealtimeEvent::Error {
                    error_type: error
                        .get("type")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    code: error
                        .get("code")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                });
            }
            "conversation.item.input_audio_transcription.failed" => {
                mapped.push(RealtimeEvent::Error {
                    error_type: Some("transcription_failed".to_string()),
                    code: None,
                });
            }
            _ => {}
        }
        mapped
    }

    fn ready_for_output(&mut self, mapped: &mut Vec<RealtimeEvent>) {
        self.response_active = true;
        if matches!(self.adapter, RealtimeAdapter::Xai(_))
            && let Some(event) = self.finalize_input()
        {
            mapped.push(event);
        }
    }

    fn finalize_input(&mut self) -> Option<RealtimeEvent> {
        let text = std::mem::take(&mut self.input_pending);
        (!text.trim().is_empty()).then_some(RealtimeEvent::InputTranscript {
            text,
            final_text: true,
        })
    }
}

enum SessionCommand {
    Speak {
        text: String,
        voice: Option<String>,
        instructions: Option<String>,
    },
}

/// A live full-duplex realtime provider session.
#[derive(Clone)]
pub struct RealtimeSession {
    input_tx: mpsc::Sender<Vec<u8>>,
    command_tx: mpsc::Sender<SessionCommand>,
    close: Arc<Notify>,
    closed: watch::Receiver<bool>,
}

impl RealtimeSession {
    /// Opens the websocket and configures a PCMU server-VAD session.
    pub async fn connect(
        adapter: RealtimeAdapter,
        sink: Arc<dyn RealtimeAudioSink>,
        events: mpsc::UnboundedSender<RealtimeEvent>,
    ) -> Result<Self> {
        let request = adapter.websocket_request()?;
        let mut socket = connect_with_retry(request).await?;
        send_json(&mut socket, adapter.session_update()).await?;

        let (input_tx, input_rx) = mpsc::channel(256);
        let (command_tx, command_rx) = mpsc::channel(32);
        let (closed_tx, closed_rx) = watch::channel(false);
        let close = Arc::new(Notify::new());
        let close_signal = Arc::clone(&close);
        tokio::spawn(async move {
            let result = run_session(
                adapter,
                socket,
                sink.clone(),
                events.clone(),
                input_rx,
                command_rx,
                close_signal,
            )
            .await;
            if result.is_err() {
                sink.flush();
                let _ = events.send(RealtimeEvent::Error {
                    error_type: Some("transport".to_string()),
                    code: None,
                });
            }
            let _ = closed_tx.send(true);
        });
        Ok(Self {
            input_tx,
            command_tx,
            close,
            closed: closed_rx,
        })
    }

    /// Streams one caller PCMU payload into the provider input buffer.
    pub async fn send_audio(&self, audio: Vec<u8>) -> Result<()> {
        self.input_tx
            .send(audio)
            .await
            .map_err(|_| anyhow!("realtime input channel closed"))
    }

    pub(crate) fn try_send_audio(
        &self,
        audio: Vec<u8>,
    ) -> std::result::Result<(), mpsc::error::TrySendError<Vec<u8>>> {
        self.input_tx.try_send(audio)
    }

    /// Requests provider speech for the supplied text.
    pub async fn speak(
        &self,
        text: String,
        voice: Option<String>,
        instructions: Option<String>,
    ) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Speak {
                text,
                voice,
                instructions,
            })
            .await
            .map_err(|_| anyhow!("realtime command channel closed"))
    }

    /// Asks the realtime task to stop without blocking the caller.
    pub fn close(&self) {
        self.close.notify_one();
    }

    /// Waits until the realtime task has stopped.
    pub async fn wait_closed(&self) {
        let mut closed = self.closed.clone();
        if *closed.borrow() {
            return;
        }
        while closed.changed().await.is_ok() {
            if *closed.borrow() {
                return;
            }
        }
    }
}

async fn connect_with_retry(request: Request<()>) -> Result<RealtimeSocket> {
    let mut last_error = None;
    for attempt in 0..3 {
        match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request.clone())).await {
            Ok(Ok((socket, _))) => return Ok(socket),
            Ok(Err(error)) => last_error = Some(anyhow!(error)),
            Err(_) => last_error = Some(anyhow!("realtime websocket connect timed out")),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(100 * (attempt + 1) as u64)).await;
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("failed to connect realtime websocket")))
}

type RealtimeSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Default)]
struct QueuedAudio {
    bytes: VecDeque<u8>,
    item_id: Option<String>,
    generated_bytes: u64,
    flushed_bytes: u64,
    finished: bool,
}

impl QueuedAudio {
    fn push(&mut self, item_id: Option<String>, audio: &[u8]) {
        if item_id != self.item_id {
            self.item_id = item_id;
            self.generated_bytes = 0;
            self.flushed_bytes = 0;
        }
        self.generated_bytes = self.generated_bytes.saturating_add(audio.len() as u64);
        self.bytes.extend(audio);
        self.finished = false;
    }

    fn discard_overflow(&mut self) -> usize {
        let excess = self.bytes.len().saturating_sub(MAX_QUEUED_OUTPUT_BYTES);
        let frames = excess.div_ceil(PCMU_BYTES_PER_FRAME);
        let dropped = (frames * PCMU_BYTES_PER_FRAME).min(self.bytes.len());
        self.bytes.drain(..dropped);
        self.flushed_bytes = self.flushed_bytes.saturating_add(dropped as u64);
        dropped
    }

    fn finish(&mut self) {
        self.finished = true;
    }

    fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.bytes.len() < PCMU_BYTES_PER_FRAME {
            if !self.finished || self.bytes.is_empty() {
                return None;
            }
            self.bytes.extend(std::iter::repeat_n(
                PCMU_SILENCE,
                PCMU_BYTES_PER_FRAME - self.bytes.len(),
            ));
        }
        Some(self.bytes.drain(..PCMU_BYTES_PER_FRAME).collect())
    }

    fn flush_for_barge_in(&mut self) -> u64 {
        let unplayed = (self.bytes.len() as u64).saturating_add(self.flushed_bytes);
        let played = self.generated_bytes.saturating_sub(unplayed);
        self.flushed_bytes = unplayed;
        self.bytes.clear();
        played / 8
    }
}

async fn send_json(socket: &mut RealtimeSocket, value: Value) -> Result<()> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .context("failed to send realtime websocket event")
}

async fn run_session(
    adapter: RealtimeAdapter,
    mut socket: RealtimeSocket,
    sink: Arc<dyn RealtimeAudioSink>,
    events: mpsc::UnboundedSender<RealtimeEvent>,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
    mut command_rx: mpsc::Receiver<SessionCommand>,
    close: Arc<Notify>,
) -> Result<()> {
    let mut mapper = EventMapper::new(adapter.clone());
    let mut output = QueuedAudio::default();
    let mut output_marker = true;
    let mut command_open = true;
    let mut dropped_output_bytes = 0;
    let mut next_drop_log = Instant::now();
    let mut ticker = interval(OUTPUT_FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = close.notified() => {
                let _ = socket.close(None).await;
                return Ok(());
            }
            input = input_rx.recv() => {
                let Some(audio) = input else {
                    let _ = socket.close(None).await;
                    return Ok(());
                };
                send_json(&mut socket, json!({
                    "type": "input_audio_buffer.append",
                    "audio": BASE64.encode(audio)
                })).await?;
            }
            command = command_rx.recv(), if command_open => {
                match command {
                    Some(SessionCommand::Speak { text, voice, instructions }) => {
                        if text.trim().is_empty() {
                            continue;
                        }
                        mapper.response_active = true;
                        output_marker = true;
                        for event in
                            adapter.speak_events(&text, voice.as_deref(), instructions.as_deref())
                        {
                            send_json(&mut socket, event).await?;
                        }
                    }
                    None => command_open = false,
                }
            }
            message = socket.next() => {
                let value: Value = match message {
                    Some(Ok(Message::Text(payload))) => serde_json::from_str(&payload)
                        .context("provider returned invalid realtime JSON")?,
                    Some(Ok(Message::Binary(payload))) => serde_json::from_slice(&payload)
                        .context("provider returned invalid realtime JSON")?,
                    Some(Ok(Message::Ping(payload))) => {
                        socket.send(Message::Pong(payload)).await
                            .context("failed to send realtime websocket pong")?;
                        continue;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(anyhow!("realtime websocket closed unexpectedly"));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => {
                        return Err(error).context("realtime websocket read failed");
                    }
                };
                let event_type = value.get("type").and_then(Value::as_str);
                if event_type == Some("response.created")
                    || (event_type == Some("response.output_audio.delta")
                        && !mapper.response_active)
                {
                    output_marker = true;
                }
                for event in mapper.map(&value) {
                    match event {
                        RealtimeEvent::AudioDelta { audio, item_id } => {
                            output.push(item_id, &audio);
                            let dropped = output.discard_overflow();
                            if dropped > 0 {
                                dropped_output_bytes += dropped;
                                let now = Instant::now();
                                if now >= next_drop_log {
                                    warn!(
                                        dropped_bytes = dropped_output_bytes,
                                        "dropped oldest realtime output frames because the queue was full"
                                    );
                                    dropped_output_bytes = 0;
                                    next_drop_log = now + DROP_LOG_INTERVAL;
                                }
                            }
                        }
                        RealtimeEvent::SpeechStarted => {
                            let audio_end_ms = output.flush_for_barge_in();
                            sink.flush();
                            output_marker = true;
                            if mapper.response_active {
                                send_json(&mut socket, json!({"type": "response.cancel"})).await?;
                                if let Some(item_id) = output.item_id.as_deref()
                                    && audio_end_ms > 0
                                {
                                    send_json(&mut socket, json!({
                                        "type": "conversation.item.truncate",
                                        "item_id": item_id,
                                        "content_index": 0,
                                        "audio_end_ms": audio_end_ms
                                    })).await?;
                                }
                                mapper.response_active = false;
                            }
                            let _ = events.send(RealtimeEvent::SpeechStarted);
                        }
                        other => {
                            let _ = events.send(other);
                        }
                    }
                }
                if matches!(
                    event_type,
                    Some("response.output_audio.done" | "response.done")
                ) {
                    output.finish();
                }
            }
            _ = ticker.tick() => {
                if let Some(frame) = output.next_frame() {
                    sink.send_audio(&frame, output_marker)?;
                    output_marker = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use parking_lot::Mutex;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    fn config(provider: &str) -> RealtimeConnectionConfig {
        RealtimeConnectionConfig {
            endpoint: "wss://example.test/v1/realtime".to_string(),
            api_key: "sk-test".to_string(),
            model: if provider == "xai" {
                "grok-voice-latest"
            } else {
                "gpt-realtime-2.1"
            }
            .to_string(),
            voice: if provider == "xai" { "eve" } else { "marin" }.to_string(),
            instructions: Some("Be brief.".to_string()),
            transcription_model: Some(
                if provider == "xai" {
                    "grok-transcribe"
                } else {
                    "gpt-4o-transcribe"
                }
                .to_string(),
            ),
            language_hint: (provider == "xai").then(|| "en-AU".to_string()),
            reasoning_effort: (provider == "xai").then(|| "none".to_string()),
        }
    }

    fn adapter(provider: &str) -> RealtimeAdapter {
        if provider == "xai" {
            RealtimeAdapter::Xai(config(provider))
        } else {
            RealtimeAdapter::OpenAi(config(provider))
        }
    }

    #[test]
    fn session_updates_use_pcmu_and_provider_shapes() {
        let openai = adapter("openai").session_update();
        assert_eq!(openai["session"]["type"], "realtime");
        assert_eq!(
            openai["session"]["audio"]["input"]["format"]["type"],
            "audio/pcmu"
        );
        assert_eq!(openai["session"]["audio"]["output"]["voice"], "marin");

        let xai = adapter("xai").session_update();
        assert!(xai["session"].get("type").is_none());
        assert_eq!(xai["session"]["reasoning"]["effort"], "none");
        assert_eq!(
            xai["session"]["audio"]["input"]["transcription"]["language_hint"],
            "en-AU"
        );
        assert_eq!(
            xai["session"]["audio"]["output"]["format"]["type"],
            "audio/pcmu"
        );
    }

    #[test]
    fn xai_cumulative_transcript_finalizes_at_response_start() {
        let mut mapper = EventMapper::new(adapter("xai"));
        let first = mapper.map(&json!({
            "type": "conversation.item.input_audio_transcription.updated",
            "transcript": "Book"
        }));
        assert!(matches!(
            first.first(),
            Some(RealtimeEvent::InputTranscript { text, final_text: false }) if text == "Book"
        ));
        let second = mapper.map(&json!({
            "type": "conversation.item.input_audio_transcription.updated",
            "transcript": "Book Friday"
        }));
        assert!(matches!(
            second.first(),
            Some(RealtimeEvent::InputTranscript { text, final_text: false }) if text == "Book Friday"
        ));
        let started = mapper.map(&json!({"type": "response.created"}));
        assert!(matches!(
            started.first(),
            Some(RealtimeEvent::InputTranscript { text, final_text: true }) if text == "Book Friday"
        ));
    }

    #[test]
    fn provider_events_map_and_unknown_events_are_ignored() {
        let mut mapper = EventMapper::new(adapter("openai"));
        let input_delta = mapper.map(&json!({
            "type": "conversation.item.input_audio_transcription.delta",
            "delta": "Book"
        }));
        assert!(matches!(
            input_delta.first(),
            Some(RealtimeEvent::InputTranscript { text, final_text: false }) if text == "Book"
        ));
        let input_done = mapper.map(&json!({
            "type": "conversation.item.input_audio_transcription.completed",
            "transcript": "Book Friday"
        }));
        assert!(matches!(
            input_done.first(),
            Some(RealtimeEvent::InputTranscript { text, final_text: true }) if text == "Book Friday"
        ));
        let audio = BASE64.encode([1_u8, 2, 3]);
        let mapped = mapper.map(&json!({
            "type": "response.output_audio.delta",
            "delta": audio
        }));
        assert!(matches!(
            mapped.first(),
            Some(RealtimeEvent::AudioDelta { audio, .. }) if audio == &[1, 2, 3]
        ));
        let output_delta = mapper.map(&json!({
            "type": "response.output_audio_transcript.delta",
            "delta": "Friday"
        }));
        assert!(matches!(
            output_delta.first(),
            Some(RealtimeEvent::OutputTranscript { text, final_text: false }) if text == "Friday"
        ));
        let output_done = mapper.map(&json!({
            "type": "response.output_audio_transcript.done",
            "transcript": "Friday"
        }));
        assert!(matches!(
            output_done.first(),
            Some(RealtimeEvent::OutputTranscript { text, final_text: true }) if text == "Friday"
        ));
        assert!(matches!(
            mapper
                .map(&json!({"type": "input_audio_buffer.speech_started"}))
                .first(),
            Some(RealtimeEvent::SpeechStarted)
        ));
        assert!(matches!(
            mapper
                .map(&json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "code": "bad_request",
                        "message": "provider secret detail"
                    }
                }))
                .first(),
            Some(RealtimeEvent::Error {
                error_type: Some(error_type),
                code: Some(code),
            }) if error_type == "invalid_request_error" && code == "bad_request"
        ));
        assert!(
            mapper
                .map(&json!({"type": "conversation.item.done"}))
                .is_empty()
        );
        let done = mapper.map(&json!({
            "type": "response.done",
            "response": {
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "input_token_details": {"audio_tokens": 7, "text_tokens": 3},
                    "output_token_details": {"audio_tokens": 2, "text_tokens": 2}
                }
            }
        }));
        assert!(matches!(
            done.last(),
            Some(RealtimeEvent::ResponseDone { usage: Some(usage) })
                if usage.input_audio_tokens == 7 && usage.output_audio_tokens == 2
        ));
    }

    #[test]
    fn debug_redacts_connection_key() {
        let debug = format!("{:?}", adapter("xai"));
        assert!(!debug.contains("sk-test"));
    }

    #[test]
    fn queued_audio_pads_complete_frames_and_tracks_barge_in_audio() {
        let mut output = QueuedAudio::default();
        output.push(Some("item-a".to_string()), &[1; 161]);
        assert_eq!(
            output.next_frame().unwrap(),
            std::iter::repeat_n(1, PCMU_BYTES_PER_FRAME).collect::<Vec<_>>()
        );
        output.finish();
        let frame = output.next_frame().unwrap();
        assert_eq!(frame[0], 1);
        assert!(frame[1..].iter().all(|byte| *byte == PCMU_SILENCE));

        output.push(Some("item-b".to_string()), &[2; 320]);
        assert_eq!(output.generated_bytes, 320);
        assert_eq!(output.next_frame().unwrap(), vec![2; PCMU_BYTES_PER_FRAME]);
        assert_eq!(output.flush_for_barge_in(), 20);
        output.push(Some("item-b".to_string()), &[3; 160]);
        assert_eq!(output.generated_bytes, 480);
        assert_eq!(output.flush_for_barge_in(), 20);
    }

    #[test]
    fn queued_audio_overflow_drops_whole_frames() {
        let mut output = QueuedAudio::default();
        output.push(
            Some("item-a".to_string()),
            &vec![1; MAX_QUEUED_OUTPUT_BYTES + 1],
        );
        assert_eq!(output.discard_overflow(), PCMU_BYTES_PER_FRAME);
        assert_eq!(
            output.bytes.len(),
            MAX_QUEUED_OUTPUT_BYTES - PCMU_BYTES_PER_FRAME + 1
        );
        assert_eq!(output.flushed_bytes, PCMU_BYTES_PER_FRAME as u64);
    }

    #[test]
    fn websocket_request_uses_provider_bearer_and_model_query() {
        let request = adapter("xai").websocket_request().unwrap();
        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer sk-test"
        );
        assert_eq!(request.uri().query(), Some("model=grok-voice-latest"));
    }

    #[derive(Default)]
    struct RecordingSink {
        audio: Mutex<Vec<Vec<u8>>>,
        flushes: AtomicUsize,
    }

    impl RealtimeAudioSink for RecordingSink {
        fn send_audio(&self, audio: &[u8], _marker: bool) -> Result<()> {
            self.audio.lock().push(audio.to_vec());
            Ok(())
        }

        fn flush(&self) {
            self.flushes.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct MockServer {
        task: tokio::task::JoinHandle<()>,
        to_client: mpsc::Sender<Value>,
        from_client: mpsc::Receiver<Value>,
    }

    async fn start_mock_server() -> (String, MockServer) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (to_client, mut to_client_rx): (mpsc::Sender<Value>, mpsc::Receiver<Value>) =
            mpsc::channel(16);
        let (from_client_tx, from_client): (mpsc::Sender<Value>, mpsc::Receiver<Value>) =
            mpsc::channel(16);
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let first = socket.next().await.unwrap().unwrap();
            let Message::Text(first) = first else {
                panic!("expected text session update");
            };
            let first: Value = serde_json::from_str(&first).unwrap();
            assert_eq!(first["type"], "session.update");
            from_client_tx.send(first).await.unwrap();
            loop {
                tokio::select! {
                    outgoing = to_client_rx.recv() => {
                        let Some(outgoing) = outgoing else { break };
                        socket.send(Message::Text(outgoing.to_string().into())).await.unwrap();
                    }
                    incoming = socket.next() => {
                        let Some(incoming) = incoming else { break };
                        let incoming = match incoming.unwrap() {
                            Message::Text(text) => serde_json::from_str(&text).unwrap(),
                            Message::Close(_) => break,
                            _ => continue,
                        };
                        from_client_tx.send(incoming).await.unwrap();
                    }
                }
            }
        });
        (
            format!("ws://{address}/v1/realtime"),
            MockServer {
                task,
                to_client,
                from_client,
            },
        )
    }

    async fn next_type(server: &mut MockServer, expected: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = server.from_client.recv().await.expect("client event");
                if event["type"] == expected {
                    return event;
                }
            }
        })
        .await
        .unwrap()
    }

    async fn exercise_provider(provider: &str) {
        let mut config = config(provider);
        let (endpoint, mut server) = start_mock_server().await;
        config.endpoint = endpoint;
        let adapter = if provider == "xai" {
            RealtimeAdapter::Xai(config)
        } else {
            RealtimeAdapter::OpenAi(config)
        };
        let sink = Arc::new(RecordingSink::default());
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let session = RealtimeSession::connect(adapter, sink.clone(), events_tx)
            .await
            .unwrap();

        let update = server.from_client.recv().await.unwrap();
        assert_eq!(
            update["session"]["audio"]["input"]["format"]["type"],
            "audio/pcmu"
        );

        session.send_audio(vec![1, 2, 3]).await.unwrap();
        let append = next_type(&mut server, "input_audio_buffer.append").await;
        assert_eq!(
            BASE64.decode(append["audio"].as_str().unwrap()).unwrap(),
            vec![1, 2, 3]
        );

        server
            .to_client
            .send(json!({"type": "response.created"}))
            .await
            .unwrap();
        server
            .to_client
            .send(json!({
                "type": "response.output_item.added",
                "item": {"id": "item-audio"}
            }))
            .await
            .unwrap();
        let audio = vec![8_u8; PCMU_BYTES_PER_FRAME];
        server
            .to_client
            .send(json!({
                "type": "response.output_audio.delta",
                "delta": BASE64.encode(&audio)
            }))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !sink.audio.lock().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(sink.audio.lock()[0], audio);

        server
            .to_client
            .send(json!({"type": "input_audio_buffer.speech_started"}))
            .await
            .unwrap();
        next_type(&mut server, "response.cancel").await;
        let truncate = next_type(&mut server, "conversation.item.truncate").await;
        assert_eq!(truncate["item_id"], "item-audio");
        assert_eq!(truncate["audio_end_ms"], 20);
        assert_eq!(sink.flushes.load(Ordering::SeqCst), 1);

        session
            .speak(
                "Hello".to_string(),
                Some("test-voice".to_string()),
                Some("Be calm.".to_string()),
            )
            .await
            .unwrap();
        if provider == "xai" {
            let speak = next_type(&mut server, "conversation.item.create").await;
            assert_eq!(speak["item"]["type"], "force_message");
            assert_eq!(speak["item"]["content"][0]["text"], "Hello");
        } else {
            let item = next_type(&mut server, "conversation.item.create").await;
            assert_eq!(item["item"]["type"], "message");
            assert!(
                item["item"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Hello")
            );
            let speak = next_type(&mut server, "response.create").await;
            assert!(
                speak["response"]["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("Hello")
            );
        }

        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(2), events_rx.recv()).await;
        server.task.abort();
    }

    #[tokio::test]
    async fn mock_websocket_round_trip_openai() {
        exercise_provider("openai").await;
    }

    #[tokio::test]
    async fn mock_websocket_round_trip_xai() {
        exercise_provider("xai").await;
    }
}
