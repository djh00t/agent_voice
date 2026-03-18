//! Runtime SIP call orchestration, media handling, and local control surface.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};

use anyhow::{Context, Result, anyhow};
use chrono::{Timelike, Utc};
use chrono_tz::Tz;
use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};
use xphone::{Call, DialOptions, Phone};

use crate::accounting::{
    AccountingStore, ApiCallContext, ApiCallLogEntry, CallAccountingSummary, CallTotalsLogEntry,
    ModelUsageSummary, TokenUsage,
};
use crate::api;
use crate::audio::{TELEPHONY_FRAME_SAMPLES, TELEPHONY_RATE, encode_wav_mono_i16};
use crate::config::{AppConfig, BehaviorConfig};
use crate::llm::LlmService;
use crate::openai::{ConversationContext, OpenAiClients, TranscriptEvent, TranscriptSink};
use crate::phonebook::{
    CallerUpdate, PhoneBookStore, caller_id_display, is_valid_timezone, normalize_caller_id,
    normalize_email_candidate,
};
use crate::sherpa_onnx::SherpaOnnxClient;
use crate::stt::SttService;
use crate::tts::{SynthesisOutcome, TtsService};
use crate::voice::VoiceService;

#[derive(Clone)]
/// The long-running SIP voice service used by the local agent API.
pub struct VoiceAgentService {
    config: AppConfig,
    phone: Phone,
    llm: LlmService,
    stt: SttService,
    tts: TtsService,
    voice: VoiceService,
    phone_book: Arc<PhoneBookStore>,
    accounting: Arc<AccountingStore>,
    state: Arc<ServiceState>,
}

impl VoiceAgentService {
    /// Constructs a new voice service from fully resolved configuration.
    pub async fn new(config: AppConfig) -> Result<Self> {
        let phone = Phone::new(config.phone_config());
        let openai = OpenAiClients::new(config.openai.clone())?;
        let sherpa = SherpaOnnxClient::new(
            config.speech.sherpa_onnx.clone(),
            config.speech.uses_local_stt(),
            config.speech.uses_local_tts(),
        )
        .await?;
        let llm = LlmService::new(config.llm.clone(), openai.clone());
        let stt = SttService::new(config.speech.clone(), openai.clone(), sherpa.clone());
        let tts = TtsService::new(config.speech.clone(), openai.clone(), sherpa);
        let voice = VoiceService::new(config.voice.clone(), openai);
        let phone_book = Arc::new(PhoneBookStore::load(&config.behavior.phone_book_path)?);
        let accounting = Arc::new(AccountingStore::load(&config.accounting)?);
        let mut required_models = Vec::new();
        if config.speech.uses_openai_stt() {
            required_models.push(config.openai.transcription_model.as_str());
        }
        if config.speech.uses_openai_tts() {
            required_models.push(config.openai.tts_model.as_str());
        }
        if config.llm.uses_openai() {
            required_models.push(config.openai.response_model.as_str());
        }
        if config.voice.uses_openai() {
            required_models.push(config.voice.openai.model.as_str());
        }
        accounting.validate_required_models(required_models)?;
        let state = Arc::new(ServiceState::default());
        Ok(Self {
            config,
            phone,
            llm,
            stt,
            tts,
            voice,
            phone_book,
            accounting,
            state,
        })
    }

    /// Runs the SIP phone and local HTTP control API until shutdown.
    pub async fn run(self) -> Result<()> {
        let service = Arc::new(self);
        service.register_callbacks();
        service
            .phone
            .connect()
            .context("failed to register SIP phone")?;

        let app = api::router(Arc::clone(&service));
        let listen_addr: SocketAddr = service
            .config
            .agent_api
            .listen
            .parse()
            .with_context(|| "invalid agent_api.listen address")?;
        let listener = TcpListener::bind(listen_addr)
            .await
            .with_context(|| format!("failed to bind {}", listen_addr))?;
        info!(listen = %listen_addr, "agent voice API listening");

        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            .context("HTTP server exited unexpectedly")?;

        if let Err(error) = service.phone.disconnect() {
            warn!(error = %error, "phone disconnect returned error during shutdown");
        }
        Ok(())
    }

    /// Returns the current service-wide SIP and call status snapshot.
    pub fn status(&self) -> ServiceStatus {
        ServiceStatus {
            phone_state: self.phone.state().to_string(),
            conversation_mode: if self.voice.is_enabled() {
                "voice_model".to_string()
            } else {
                "split".to_string()
            },
            stt_backend: self.stt.backend_name().to_string(),
            stt_model: self.stt.model_name(),
            tts_backend: self.tts.backend_name().to_string(),
            tts_model: self.tts.model_name(),
            llm_backend: self.llm.backend_name().to_string(),
            llm_model: self.llm.model_name(),
            voice_backend: self.voice.backend_name().to_string(),
            voice_model: self.voice.model_name(),
            calls: self
                .state
                .calls
                .read()
                .values()
                .map(|call| call.snapshot())
                .collect(),
        }
    }

    /// Places an outbound SIP call to the provided target URI.
    pub async fn dial(&self, target: String) -> Result<CallSnapshot> {
        let call = self
            .phone
            .dial(&target, DialOptions::default())
            .map_err(|error| anyhow!("failed to dial {}: {}", target, error))?;
        self.bootstrap_call(call, "outbound", normalize_caller_id(&target), true)
            .await
    }

    /// Returns snapshots for all currently tracked calls.
    pub fn list_calls(&self) -> Vec<CallSnapshot> {
        self.state
            .calls
            .read()
            .values()
            .map(|call| call.snapshot())
            .collect()
    }

    /// Returns a snapshot for a single call when it exists.
    pub fn call_snapshot(&self, call_id: &str) -> Option<CallSnapshot> {
        self.state
            .calls
            .read()
            .get(call_id)
            .map(|call| call.snapshot())
    }

    /// Returns the persisted in-memory transcript history for a call.
    pub fn transcript_for(&self, call_id: &str) -> Option<Vec<TranscriptEvent>> {
        self.state
            .calls
            .read()
            .get(call_id)
            .map(|call| call.transcript_events.read().clone())
    }

    /// Speaks arbitrary text into an active call through OpenAI TTS.
    pub async fn speak_text(
        &self,
        call_id: &str,
        text: String,
        voice: Option<String>,
        instructions: Option<String>,
    ) -> Result<()> {
        let call = self
            .state
            .calls
            .read()
            .get(call_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown call id {}", call_id))?;
        let tx = call
            .speaker_tx
            .read()
            .clone()
            .ok_or_else(|| anyhow!("call media is not ready yet"))?;
        let summary = queue_tts_text(
            &self.tts,
            &self.accounting,
            &call,
            &tx,
            QueuedTtsRequest {
                text: &text,
                operation: "tts.manual",
                post_tts_input_suppression_ms: self.config.behavior.post_tts_input_suppression_ms,
                voice,
                instructions,
            },
        )
        .await?;
        info!(
            call_id = %call.call.call_id(),
            backend = self.tts.backend_name(),
            total_tts_ms = summary.total_tts_ms,
            tts_first_audio_ms = summary.first_audio_ms,
            total_playback_ms = summary.total_playback_ms,
            segment_count = summary.segment_count,
            "manual assistant audio queued for RTP playback"
        );
        Ok(())
    }

    /// Ends an active SIP call by sending `BYE`.
    pub fn hangup(&self, call_id: &str) -> Result<()> {
        let call = self
            .state
            .calls
            .read()
            .get(call_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown call id {}", call_id))?;
        call.call.end().map_err(|error| anyhow!(error.to_string()))
    }

    fn register_callbacks(self: &Arc<Self>) {
        let runtime = tokio::runtime::Handle::current();
        let service = Arc::clone(self);
        self.phone.on_registered(move || {
            info!("registered with SIP server");
            service
                .state
                .phone_registered
                .store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let service = Arc::clone(self);
        self.phone.on_unregistered(move || {
            info!("unregistered from SIP server");
            service
                .state
                .phone_registered
                .store(false, std::sync::atomic::Ordering::Relaxed);
        });

        let runtime = runtime.clone();
        let service = Arc::clone(self);
        self.phone.on_incoming(move |call| {
            let service = Arc::clone(&service);
            let runtime = runtime.clone();
            runtime.spawn(async move {
                if let Err(error) = service.handle_incoming(call).await {
                    error!(error = %error, "failed to handle incoming call");
                }
            });
        });
    }

    async fn handle_incoming(self: Arc<Self>, call: Arc<Call>) -> Result<()> {
        let access = self.phone_book.inbound_access_decision(&call.from());
        if !access.allowed {
            let caller_label = access
                .caller_id
                .clone()
                .unwrap_or_else(|| caller_id_display(&call.from()));
            info!(
                call_id = %call.call_id(),
                caller_id = %caller_label,
                matched_record_key = %access.matched_record_key,
                "rejecting inbound call by phone book access policy"
            );
            call.reject(603, "Inbound calls are not accepted for this caller")
                .context("failed to reject inbound call")?;
            return Ok(());
        }

        if self.config.behavior.auto_answer_incoming || self.config.sip.accept_incoming_calls {
            let answer_delay_ms = self.config.behavior.incoming_answer_delay_ms;
            if answer_delay_ms > 0 {
                info!(
                    call_id = %call.call_id(),
                    delay_ms = answer_delay_ms,
                    "delaying inbound answer"
                );
                sleep(Duration::from_millis(answer_delay_ms)).await;
            }
            call.accept().context("failed to accept incoming call")?;
        }
        let snapshot = self
            .bootstrap_call(
                call,
                "inbound",
                access.caller_id,
                access.track_existing_caller,
            )
            .await?;
        if let Err(error) = self.play_incoming_greeting(&snapshot.call_id).await {
            warn!(call_id = %snapshot.call_id, error = %error, "failed to play incoming greeting");
        }
        Ok(())
    }

    async fn bootstrap_call(
        &self,
        call: Arc<Call>,
        direction: &str,
        phone_book_key: Option<String>,
        track_existing_caller: bool,
    ) -> Result<CallSnapshot> {
        let call_id = call.call_id();
        if let Some(existing) = self.state.calls.read().get(&call_id) {
            return Ok(existing.snapshot());
        }

        let record = Arc::new(ManagedCall::new(
            Arc::clone(&call),
            direction.to_string(),
            phone_book_key,
            track_existing_caller,
        ));

        self.state
            .calls
            .write()
            .insert(call_id.clone(), Arc::clone(&record));

        let record_clone = Arc::clone(&record);
        let transcript_dir = self.config.behavior.transcript_dir.clone();
        let accounting = Arc::clone(&self.accounting);
        call.on_ended(move |reason| {
            record_clone.set_status(format!("ended:{reason}"));
            if let Err(error) = record_clone.persist_transcript(&transcript_dir) {
                record_clone.mark_error(format!("failed to persist transcript: {error}"));
            }
            if let Err(error) =
                accounting.record_call_total(&record_clone.call_totals_log_entry(reason))
            {
                record_clone.mark_error(format!("failed to persist call totals: {error}"));
            }
        });

        let record_clone = Arc::clone(&record);
        call.on_state(move |state| {
            record_clone.set_status(state.to_string());
        });

        let runtime = tokio::runtime::Handle::current();
        let llm = self.llm.clone();
        let stt = self.stt.clone();
        let tts = self.tts.clone();
        let voice = self.voice.clone();
        let behavior_cfg = self.config.behavior.clone();
        let phone_book = Arc::clone(&self.phone_book);
        let accounting = Arc::clone(&self.accounting);
        let call_for_media = Arc::clone(&call);
        let record_for_media = Arc::clone(&record);
        call.on_media(move || {
            let runtime = runtime.clone();
            let llm = llm.clone();
            let stt = stt.clone();
            let tts = tts.clone();
            let voice = voice.clone();
            let behavior_cfg = behavior_cfg.clone();
            let phone_book = Arc::clone(&phone_book);
            let accounting = Arc::clone(&accounting);
            let call = Arc::clone(&call_for_media);
            let record = Arc::clone(&record_for_media);
            runtime.spawn(async move {
                let result = if voice.is_enabled() {
                    activate_voice_bridge(
                        stt,
                        llm,
                        tts,
                        voice,
                        behavior_cfg,
                        phone_book,
                        accounting,
                        record,
                        call,
                    )
                    .await
                } else {
                    activate_media_bridge(
                        llm,
                        stt,
                        tts,
                        behavior_cfg,
                        phone_book,
                        accounting,
                        record,
                        call,
                    )
                    .await
                };
                if let Err(error) = result {
                    error!(error = %error, "failed to attach media bridge");
                }
            });
        });

        if self.voice.is_enabled() {
            activate_voice_bridge(
                self.stt.clone(),
                self.llm.clone(),
                self.tts.clone(),
                self.voice.clone(),
                self.config.behavior.clone(),
                Arc::clone(&self.phone_book),
                Arc::clone(&self.accounting),
                Arc::clone(&record),
                Arc::clone(&call),
            )
            .await?;
        } else {
            activate_media_bridge(
                self.llm.clone(),
                self.stt.clone(),
                self.tts.clone(),
                self.config.behavior.clone(),
                Arc::clone(&self.phone_book),
                Arc::clone(&self.accounting),
                Arc::clone(&record),
                Arc::clone(&call),
            )
            .await?;
        }

        if record.track_existing_caller
            && let Some(phone_book_key) = record.phone_book_key.as_deref()
            && let Err(error) = self.phone_book.touch_caller(phone_book_key)
        {
            record.mark_error(format!("failed to update phone book: {error}"));
        }

        Ok(record.snapshot())
    }

    async fn play_incoming_greeting(&self, call_id: &str) -> Result<()> {
        let peer = self
            .state
            .calls
            .read()
            .get(call_id)
            .and_then(|call| call.phone_book_key.clone());
        let greeting = build_initial_greeting(
            peer.as_deref()
                .and_then(|key| self.phone_book.get(key))
                .as_ref(),
            &self.config.behavior,
        );
        if greeting.is_empty() {
            return Ok(());
        }

        for _ in 0..50 {
            let media_ready = self
                .state
                .calls
                .read()
                .get(call_id)
                .map(|call| call.speaker_tx.read().is_some())
                .unwrap_or(false);
            if media_ready {
                let caller = peer.as_deref().and_then(|key| self.phone_book.get(key));
                info!(call_id = %call_id, greeting = %greeting, "playing incoming greeting");
                return self
                    .speak_text(
                        call_id,
                        build_initial_greeting(caller.as_ref(), &self.config.behavior),
                        None,
                        None,
                    )
                    .await;
            }
            sleep(Duration::from_millis(100)).await;
        }

        Err(anyhow!("call media did not become ready for greeting"))
    }
}

#[derive(Default)]
struct ServiceState {
    calls: RwLock<HashMap<String, Arc<ManagedCall>>>,
    phone_registered: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone, serde::Serialize)]
/// A serialized snapshot of overall service state for the control API.
pub struct ServiceStatus {
    pub phone_state: String,
    pub conversation_mode: String,
    pub stt_backend: String,
    pub stt_model: String,
    pub tts_backend: String,
    pub tts_model: String,
    pub llm_backend: String,
    pub llm_model: Option<String>,
    pub voice_backend: String,
    pub voice_model: Option<String>,
    pub calls: Vec<CallSnapshot>,
}

#[derive(Debug, Clone, serde::Serialize)]
/// A serialized view of a single active or recently tracked call.
pub struct CallSnapshot {
    pub call_id: String,
    pub direction: String,
    pub peer: String,
    pub state: String,
    pub started_at: String,
    pub media_ready: bool,
    pub transcript_events: usize,
    pub api_call_count: u64,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub model_usage: Vec<ModelUsageSummary>,
    pub last_error: Option<String>,
}

struct ManagedCall {
    call: Arc<Call>,
    direction: String,
    speaker_tx: RwLock<Option<crossbeam_channel::Sender<Vec<i16>>>>,
    transcript_events: RwLock<Vec<TranscriptEvent>>,
    status: RwLock<String>,
    last_error: RwLock<Option<String>>,
    started_at: String,
    peer: String,
    phone_book_key: Option<String>,
    track_existing_caller: bool,
    bridge_started: AtomicBool,
    end_requested: AtomicBool,
    idle_watch_generation: AtomicU64,
    input_suppressed_until: RwLock<Option<Instant>>,
    pending_email_confirmation: RwLock<Option<String>>,
    last_reply_response_id: RwLock<Option<String>>,
    turn_stats: RwLock<TurnStats>,
    api_call_entries: RwLock<Vec<ApiCallLogEntry>>,
    call_accounting: RwLock<CallAccountingSummary>,
}

impl ManagedCall {
    fn new(
        call: Arc<Call>,
        direction: String,
        phone_book_key: Option<String>,
        track_existing_caller: bool,
    ) -> Self {
        let raw_peer = if direction == "inbound" {
            call.from()
        } else {
            call.to()
        };
        let peer = phone_book_key.clone().unwrap_or_else(|| {
            if direction == "inbound" {
                caller_id_display(&raw_peer)
            } else {
                raw_peer
            }
        });
        Self {
            call,
            direction,
            speaker_tx: RwLock::new(None),
            transcript_events: RwLock::new(Vec::new()),
            status: RwLock::new("pending-media".to_string()),
            last_error: RwLock::new(None),
            started_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string()),
            peer,
            phone_book_key,
            track_existing_caller,
            bridge_started: AtomicBool::new(false),
            end_requested: AtomicBool::new(false),
            idle_watch_generation: AtomicU64::new(0),
            input_suppressed_until: RwLock::new(None),
            pending_email_confirmation: RwLock::new(None),
            last_reply_response_id: RwLock::new(None),
            turn_stats: RwLock::new(TurnStats::default()),
            api_call_entries: RwLock::new(Vec::new()),
            call_accounting: RwLock::new(CallAccountingSummary::default()),
        }
    }

    fn snapshot(&self) -> CallSnapshot {
        let accounting = self.call_accounting.read().clone();
        CallSnapshot {
            call_id: self.call.call_id(),
            direction: self.direction.clone(),
            peer: self.peer.clone(),
            state: self.status.read().clone(),
            started_at: self.started_at.clone(),
            media_ready: self.speaker_tx.read().is_some(),
            transcript_events: self.transcript_events.read().len(),
            api_call_count: accounting.api_call_count,
            total_cost_usd: accounting.total_cost_usd,
            total_tokens: accounting.totals.total_tokens(),
            model_usage: accounting.model_usage,
            last_error: self.last_error.read().clone(),
        }
    }

    fn set_speaker_tx(&self, speaker_tx: crossbeam_channel::Sender<Vec<i16>>) {
        *self.speaker_tx.write() = Some(speaker_tx);
    }

    fn set_status(&self, status: String) {
        *self.status.write() = status;
    }

    fn persist_transcript(&self, transcript_dir: &str) -> Result<()> {
        fs::create_dir_all(transcript_dir)?;
        let safe_call_id = sanitize_file_component(&self.call.call_id());
        let base = format!(
            "{}/{}_{}",
            transcript_dir,
            self.started_at_safe(),
            safe_call_id
        );
        let events = self.transcript_events.read().clone();
        let summary = self.snapshot();
        let api_calls = self.api_call_entries.read().clone();

        let transcript_lines = events
            .iter()
            .filter(|event| {
                event.kind == "assistant.tts"
                    || event.kind == "conversation.item.input_audio_transcription.completed"
                    || event.kind == "caller.transcript.completed"
            })
            .map(|event| format!("[{}] {}: {}", event.at, event.role, event.text))
            .collect::<Vec<_>>();

        fs::write(format!("{base}.json"), serde_json::to_vec_pretty(&events)?)?;
        fs::write(
            format!("{base}.summary.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "call": summary,
                "api_calls": api_calls,
            }))?,
        )?;
        fs::write(format!("{base}.txt"), transcript_lines.join("\n") + "\n")?;
        Ok(())
    }

    fn record_assistant_text(&self, text: String) {
        self.transcript_events.write().push(TranscriptEvent {
            role: "assistant".to_string(),
            kind: "assistant.tts".to_string(),
            text,
            at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string()),
        });
    }

    fn record_caller_text(&self, text: String) {
        self.transcript_events.write().push(TranscriptEvent {
            role: "caller".to_string(),
            kind: "caller.transcript.completed".to_string(),
            text,
            at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string()),
        });
    }

    fn transcript_history(&self) -> Vec<TranscriptEvent> {
        self.transcript_events.read().clone()
    }

    fn record_api_call(&self, entry: ApiCallLogEntry) {
        self.api_call_entries.write().push(entry.clone());
        self.call_accounting.write().record(&entry);
    }

    fn accounting_summary(&self) -> CallAccountingSummary {
        self.call_accounting.read().clone()
    }

    fn suppress_input_for(&self, duration: StdDuration) {
        let until = Instant::now() + duration;
        let mut suppressed_until = self.input_suppressed_until.write();
        if suppressed_until
            .map(|existing| existing < until)
            .unwrap_or(true)
        {
            *suppressed_until = Some(until);
        }
    }

    fn input_is_suppressed(&self) -> bool {
        self.input_suppressed_until
            .read()
            .map(|until| Instant::now() < until)
            .unwrap_or(false)
    }

    fn pending_email_confirmation(&self) -> Option<String> {
        self.pending_email_confirmation.read().clone()
    }

    fn set_pending_email_confirmation(&self, email: Option<String>) {
        *self.pending_email_confirmation.write() = email;
    }

    fn note_activity(&self) -> u64 {
        self.idle_watch_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn idle_generation(&self) -> u64 {
        self.idle_watch_generation.load(Ordering::SeqCst)
    }

    fn last_reply_response_id(&self) -> Option<String> {
        self.last_reply_response_id.read().clone()
    }

    fn set_last_reply_response_id(&self, response_id: Option<String>) {
        *self.last_reply_response_id.write() = response_id;
    }

    fn call_totals_log_entry(&self, ended_reason: impl ToString) -> CallTotalsLogEntry {
        let accounting = self.call_accounting.read().clone();
        CallTotalsLogEntry {
            ended_at: rfc3339_now(),
            call_id: self.call.call_id(),
            direction: self.direction.clone(),
            peer: self.peer.clone(),
            started_at: self.started_at.clone(),
            ended_reason: ended_reason.to_string(),
            transcript_events: self.transcript_events.read().len(),
            api_call_count: accounting.api_call_count,
            total_cost_usd: accounting.total_cost_usd,
            input_text_tokens: accounting.totals.input_text_tokens,
            cached_input_text_tokens: accounting.totals.cached_input_text_tokens,
            output_text_tokens: accounting.totals.output_text_tokens,
            input_audio_tokens: accounting.totals.input_audio_tokens,
            output_audio_tokens: accounting.totals.output_audio_tokens,
            total_tokens: accounting.totals.total_tokens(),
            model_usage_json: serde_json::to_string(&accounting.model_usage)
                .unwrap_or_else(|_| "[]".to_string()),
        }
    }

    fn record_turn_metrics(&self, metrics: &TurnMetrics) -> TurnMetricsSummary {
        let mut stats = self.turn_stats.write();
        stats.turns += 1;
        stats.last_turn_started_at = Some(metrics.turn_started_at);
        stats.last_total_ms = metrics.total_ms;
        stats.total_ms_sum += metrics.total_ms;
        stats.stt_ms_sum += metrics.stt_ms;
        stats.extraction_ms_sum += metrics.extraction_ms;
        stats.llm_ms_sum += metrics.llm_ms;
        stats.tts_ms_sum += metrics.tts_ms;
        TurnMetricsSummary {
            turn_index: stats.turns,
            avg_total_ms: stats.total_ms_sum / stats.turns as u128,
            avg_stt_ms: stats.stt_ms_sum / stats.turns as u128,
            avg_extraction_ms: stats.extraction_ms_sum / stats.turns as u128,
            avg_llm_ms: stats.llm_ms_sum / stats.turns as u128,
            avg_tts_ms: stats.tts_ms_sum / stats.turns as u128,
        }
    }

    fn started_at_safe(&self) -> String {
        sanitize_file_component(&self.started_at)
    }
}

#[derive(Debug, Default)]
struct TurnStats {
    turns: u64,
    last_turn_started_at: Option<time::OffsetDateTime>,
    last_total_ms: u128,
    total_ms_sum: u128,
    stt_ms_sum: u128,
    extraction_ms_sum: u128,
    llm_ms_sum: u128,
    tts_ms_sum: u128,
}

#[derive(Debug, Clone, Copy)]
struct TurnMetrics {
    turn_started_at: time::OffsetDateTime,
    gap_since_previous_turn_ms: Option<i128>,
    stt_ms: u128,
    extraction_ms: u128,
    llm_ms: u128,
    tts_ms: u128,
    tts_first_audio_ms: u128,
    total_ms: u128,
}

#[derive(Debug, Clone, Copy)]
struct TurnMetricsSummary {
    turn_index: u64,
    avg_total_ms: u128,
    avg_stt_ms: u128,
    avg_extraction_ms: u128,
    avg_llm_ms: u128,
    avg_tts_ms: u128,
}

#[derive(Debug, Default)]
struct SanitizedCallerUpdate {
    update: CallerUpdate,
    pending_email_confirmation: Option<String>,
}

#[derive(Clone)]
struct MediaBridgeContext {
    llm: LlmService,
    stt: SttService,
    tts: TtsService,
    voice: Option<VoiceService>,
    phone_book: Arc<PhoneBookStore>,
    accounting: Arc<AccountingStore>,
    behavior: BehaviorConfig,
}

impl TranscriptSink for ManagedCall {
    fn push_event(&self, event: TranscriptEvent) {
        self.transcript_events.write().push(event);
    }

    fn mark_error(&self, message: String) {
        *self.last_error.write() = Some(message);
    }
}

async fn activate_media_bridge(
    llm: LlmService,
    stt: SttService,
    tts: TtsService,
    behavior: BehaviorConfig,
    phone_book: Arc<PhoneBookStore>,
    accounting: Arc<AccountingStore>,
    record: Arc<ManagedCall>,
    call: Arc<Call>,
) -> Result<()> {
    activate_call_bridge(
        MediaBridgeContext {
            llm,
            stt,
            tts,
            voice: None,
            phone_book,
            accounting,
            behavior,
        },
        record,
        call,
    )
    .await
}

async fn activate_voice_bridge(
    stt: SttService,
    llm: LlmService,
    tts: TtsService,
    voice: VoiceService,
    behavior: BehaviorConfig,
    phone_book: Arc<PhoneBookStore>,
    accounting: Arc<AccountingStore>,
    record: Arc<ManagedCall>,
    call: Arc<Call>,
) -> Result<()> {
    activate_call_bridge(
        MediaBridgeContext {
            llm,
            stt,
            tts,
            voice: Some(voice),
            phone_book,
            accounting,
            behavior,
        },
        record,
        call,
    )
    .await
}

async fn activate_call_bridge(
    bridge: MediaBridgeContext,
    record: Arc<ManagedCall>,
    call: Arc<Call>,
) -> Result<()> {
    if record.bridge_started.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let pcm_rx = match call.pcm_reader() {
        Some(reader) => reader,
        None => {
            record.bridge_started.store(false, Ordering::SeqCst);
            return Ok(());
        }
    };
    let speaker_tx = match call.paced_pcm_writer() {
        Some(writer) => writer,
        None => {
            record.bridge_started.store(false, Ordering::SeqCst);
            return Ok(());
        }
    };

    record.set_speaker_tx(speaker_tx.clone());
    record.set_status(call.state().to_string());
    info!(
        call_id = %record.call.call_id(),
        stt_backend = bridge.stt.backend_name(),
        tts_backend = bridge.tts.backend_name(),
        llm_backend = bridge.llm.backend_name(),
        voice_backend = bridge.voice.as_ref().map(|voice| voice.backend_name()).unwrap_or("disabled"),
        turn_silence_ms = bridge.behavior.turn_silence_ms,
        min_utterance_ms = bridge.behavior.min_utterance_ms,
        vad_threshold = bridge.behavior.vad_threshold,
        "started conversational media workflow"
    );

    let record_clone = Arc::clone(&record);
    let runtime = tokio::runtime::Handle::current();
    let speaker_tx_clone = speaker_tx.clone();
    std::thread::spawn(move || {
        let mut turn_detector = TurnDetector::new(&bridge.behavior);
        while let Ok(frame) = pcm_rx.recv() {
            if record_clone.input_is_suppressed() {
                turn_detector.reset();
                continue;
            }
            if let Some(utterance) = turn_detector.push_frame(&frame)
                && let Err(error) = runtime.block_on(process_detected_utterance(
                    &bridge,
                    Arc::clone(&record_clone),
                    speaker_tx_clone.clone(),
                    utterance,
                ))
            {
                record_clone.mark_error(error.to_string());
            }
        }
        if let Some(utterance) = turn_detector.finish()
            && let Err(error) = runtime.block_on(process_detected_utterance(
                &bridge,
                Arc::clone(&record_clone),
                speaker_tx_clone,
                utterance,
            ))
        {
            record_clone.mark_error(error.to_string());
        }
    });

    Ok(())
}

async fn process_detected_utterance(
    bridge: &MediaBridgeContext,
    record: Arc<ManagedCall>,
    speaker_tx: crossbeam_channel::Sender<Vec<i16>>,
    utterance: Vec<i16>,
) -> Result<()> {
    let turn_started_at = time::OffsetDateTime::now_utc();
    let gap_since_previous_turn_ms = record
        .turn_stats
        .read()
        .last_turn_started_at
        .map(|previous| (turn_started_at - previous).whole_milliseconds());
    let turn_started = Instant::now();
    info!(
        call_id = %record.call.call_id(),
        backend = bridge.stt.backend_name(),
        samples = utterance.len(),
        gap_since_previous_turn_ms = ?gap_since_previous_turn_ms,
        "sending caller audio to STT"
    );
    let wav = encode_wav_mono_i16(&utterance, TELEPHONY_RATE)?;
    let stt_started = Instant::now();
    let transcription = bridge.stt.transcribe_wav(wav.clone()).await?;
    let stt_ms = stt_started.elapsed().as_millis();
    let stt_entry = record_api_call(
        &bridge.accounting,
        &record,
        LoggedApiCall {
            operation: "transcription",
            endpoint: &transcription.endpoint,
            model: &transcription.model,
            duration_ms: stt_ms,
            usage_source: transcription.usage_source,
            estimated: transcription.estimated,
            usage: transcription.usage.clone(),
        },
    )?;
    let caller_text = transcription.text.trim().to_string().trim().to_string();
    let caller_text = if caller_text.is_empty() {
        None
    } else {
        Some(caller_text)
    };
    if let Some(caller_text) = caller_text.as_deref() {
        info!(
            call_id = %record.call.call_id(),
            caller_text,
            stt_ms,
            "caller utterance transcribed"
        );
        record.record_caller_text(caller_text.to_string());
        record.note_activity();
    } else if bridge.voice.is_none() {
        info!(call_id = %record.call.call_id(), "STT returned empty caller text");
        return Ok(());
    } else {
        info!(
            call_id = %record.call.call_id(),
            stt_ms,
            "STT returned empty caller text, continuing because a voice model is enabled"
        );
    }
    let transcript_history = record.transcript_history();
    let caller_requested_end_call = caller_text
        .as_deref()
        .map(caller_requested_end_call)
        .unwrap_or(false);
    let caller_requested_immediate_hangup = caller_text
        .as_deref()
        .map(caller_requested_immediate_hangup)
        .unwrap_or(false);

    if let Some(caller_text) = caller_text.as_deref() {
        reconcile_pending_email_confirmation(&bridge.phone_book, &record, caller_text)?;
    }
    let caller_profile = record
        .phone_book_key
        .as_deref()
        .and_then(|key| bridge.phone_book.get(key));
    let context = ConversationContext {
        assistant_name: bridge.behavior.assistant_name.clone(),
        caller_id: record.peer.clone(),
        phone_book_writable: record.phone_book_key.is_some(),
        time_of_day: time_of_day_label(
            caller_profile
                .as_ref()
                .and_then(|caller| caller.timezone.as_deref())
                .unwrap_or(&bridge.behavior.default_timezone),
        ),
        missing_fields: caller_profile
            .as_ref()
            .map(|caller| {
                caller
                    .missing_fields()
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    "first_name".to_string(),
                    "last_name".to_string(),
                    "email".to_string(),
                    "company".to_string(),
                ]
            }),
        known_caller: caller_profile.clone(),
        pending_email_confirmation: record.pending_email_confirmation(),
    };
    let context_history =
        windowed_transcript(&transcript_history, bridge.behavior.context_window_events);
    let mut extraction_ms = 0_u128;
    let mut llm_ms = 0_u128;
    let mut llm_cost_usd = 0.0_f64;
    let mut chained_response = false;
    let mut response_id = None;
    let mut model_requested_end_call = false;
    let llm_endpoint = bridge
        .llm
        .endpoint()
        .unwrap_or_else(|| "local://disabled-llm".to_string());
    let llm_model = bridge
        .llm
        .model_name()
        .unwrap_or_else(|| "disabled-llm".to_string());
    if bridge.llm.is_enabled()
        && let Some(caller_text) = caller_text.as_deref()
    {
        let extraction_started = Instant::now();
        if let Some(phone_book_key) = record.phone_book_key.as_deref()
            && should_extract_caller_update(&transcript_history, &record, caller_text)
            && let Ok(update) = bridge
                .llm
                .extract_caller_update(
                    &windowed_transcript(&transcript_history, 4),
                    caller_profile.as_ref(),
                )
                .await
        {
            let extraction_usage = update.usage.clone();
            let sanitized_update = sanitize_caller_update(update.update, caller_text);
            if let Some(email) = sanitized_update.pending_email_confirmation.clone() {
                record.set_pending_email_confirmation(Some(email));
            }
            let completed_extraction_ms = extraction_started.elapsed().as_millis();
            let _ = record_api_call(
                &bridge.accounting,
                &record,
                LoggedApiCall {
                    operation: "responses.contact_extraction",
                    endpoint: &llm_endpoint,
                    model: &llm_model,
                    duration_ms: completed_extraction_ms,
                    usage_source: "api",
                    estimated: false,
                    usage: extraction_usage,
                },
            )?;
            if let Err(error) = bridge
                .phone_book
                .merge_update(phone_book_key, sanitized_update.update)
            {
                warn!(call_id = %record.call.call_id(), error = %error, "failed to persist caller update");
            }
        }
        extraction_ms = extraction_started.elapsed().as_millis();
    }

    let mut response_text: String;
    let should_end_call;
    let playback_ms;
    let idle_generation;
    let tts_ms;
    let tts_first_audio_ms;
    let tts_cost_usd;
    if let Some(voice) = &bridge.voice {
        info!(
            call_id = %record.call.call_id(),
            transcript_events = transcript_history.len(),
            context_window_events = bridge.behavior.context_window_events,
            model = ?voice.model_name(),
            "sending caller audio turn to voice model"
        );
        let voice_started = Instant::now();
        let outcome = voice
            .respond_to_wav(
                &context_history,
                caller_text.as_deref(),
                wav,
                Some(build_voice_model_instructions(&context)),
            )
            .await?;
        llm_ms = voice_started.elapsed().as_millis();
        let voice_entry = record_api_call(
            &bridge.accounting,
            &record,
            LoggedApiCall {
                operation: "voice.reply",
                endpoint: &outcome.endpoint,
                model: &outcome.model,
                duration_ms: llm_ms,
                usage_source: "api",
                estimated: false,
                usage: outcome.usage.clone(),
            },
        )?;
        llm_cost_usd = voice_entry.cost_usd;
        response_text = outcome.text.trim().to_string();
        if response_text.is_empty() {
            info!(
                call_id = %record.call.call_id(),
                "voice model returned empty assistant transcript"
            );
            return Ok(());
        }
        playback_ms = pcm_playback_ms(outcome.pcm.len());
        record.suppress_input_for(StdDuration::from_millis(
            playback_ms.saturating_add(bridge.behavior.post_tts_input_suppression_ms),
        ));
        speaker_tx
            .send(outcome.pcm)
            .map_err(|_| anyhow!("paced pcm channel closed"))?;
        record.record_assistant_text(response_text.clone());
        idle_generation = record.note_activity();
        tts_ms = 0;
        tts_first_audio_ms = llm_ms;
        tts_cost_usd = 0.0;
        should_end_call = bridge.behavior.auto_end_calls
            && (caller_requested_immediate_hangup
                || (caller_requested_end_call && looks_like_final_farewell(&response_text)));
        model_requested_end_call = should_end_call;
    } else if let Some(response_text_fast_path) = bridge
        .behavior
        .auto_end_calls
        .then(|| caller_text.as_deref().and_then(fast_path_end_call_response))
        .flatten()
    {
        info!(
            call_id = %record.call.call_id(),
            caller_text = ?caller_text,
            response_text_fast_path,
            "using direct end-call fast path without standalone LLM"
        );
        response_text = response_text_fast_path;
        let tts_summary = queue_assistant_tts(
            &bridge.tts,
            &bridge.accounting,
            &record,
            &speaker_tx,
            &response_text,
            "tts.reply",
            bridge.behavior.post_tts_input_suppression_ms,
        )
        .await?;
        playback_ms = tts_summary.total_playback_ms;
        idle_generation = tts_summary.idle_generation;
        tts_ms = tts_summary.total_tts_ms;
        tts_first_audio_ms = tts_summary.first_audio_ms;
        tts_cost_usd = tts_summary.last_entry.cost_usd;
        should_end_call = true;
    } else {
        info!(
            call_id = %record.call.call_id(),
            transcript_events = transcript_history.len(),
            context_window_events = bridge.behavior.context_window_events,
            "sending transcript history to standalone LLM"
        );
        let previous_response_id = record.last_reply_response_id();
        let llm_started = Instant::now();
        let response = bridge
            .llm
            .generate_response_with_context(
                &context_history,
                &context,
                previous_response_id.as_deref(),
            )
            .await?;
        llm_ms = llm_started.elapsed().as_millis();
        let llm_entry = record_api_call(
            &bridge.accounting,
            &record,
            LoggedApiCall {
                operation: "responses.reply",
                endpoint: &llm_endpoint,
                model: &llm_model,
                duration_ms: llm_ms,
                usage_source: "api",
                estimated: false,
                usage: response.usage.clone(),
            },
        )?;
        llm_cost_usd = llm_entry.cost_usd;
        response_text = response.text.trim().to_string();
        if bridge.behavior.auto_end_calls && caller_requested_end_call {
            response_text = finalize_end_call_response(&response_text);
        }
        model_requested_end_call = response.end_call && bridge.behavior.auto_end_calls;
        let response_has_final_farewell = looks_like_final_farewell(&response_text);
        if model_requested_end_call && !response_has_final_farewell {
            warn!(
                call_id = %record.call.call_id(),
                response_text,
                "ignoring model end_call because assistant reply is not a final closing"
            );
        }
        should_end_call = bridge.behavior.auto_end_calls
            && (caller_requested_end_call
                || (model_requested_end_call && response_has_final_farewell));
        record.set_last_reply_response_id(response.response_id.clone());
        chained_response = previous_response_id.is_some();
        response_id = response.response_id.clone();
        let tts_summary = queue_assistant_tts(
            &bridge.tts,
            &bridge.accounting,
            &record,
            &speaker_tx,
            &response_text,
            "tts.reply",
            bridge.behavior.post_tts_input_suppression_ms,
        )
        .await?;
        playback_ms = tts_summary.total_playback_ms;
        idle_generation = tts_summary.idle_generation;
        tts_ms = tts_summary.total_tts_ms;
        tts_first_audio_ms = tts_summary.first_audio_ms;
        tts_cost_usd = tts_summary.last_entry.cost_usd;
    }
    if response_text.is_empty() {
        if should_end_call {
            response_text = default_final_farewell().to_string();
        } else {
            info!(call_id = %record.call.call_id(), "LLM returned empty assistant response");
            return Ok(());
        }
    }
    info!(
        call_id = %record.call.call_id(),
        response_text,
        llm_ms,
        chained_response,
        response_id = ?response_id,
        caller_requested_end_call,
        model_requested_end_call,
        should_end_call,
        "assistant response generated"
    );
    let suppression_ms = playback_ms.saturating_add(bridge.behavior.post_tts_input_suppression_ms);
    info!(
        call_id = %record.call.call_id(),
        playback_ms,
        post_tts_input_suppression_ms = bridge.behavior.post_tts_input_suppression_ms,
        suppression_ms,
        "suppressing inbound turn detection during assistant playback"
    );
    if should_end_call {
        let end_call_buffer_ms = if caller_requested_immediate_hangup {
            bridge.behavior.end_call_buffer_ms.min(300)
        } else {
            bridge.behavior.end_call_buffer_ms
        };
        schedule_end_call(Arc::clone(&record), playback_ms, end_call_buffer_ms);
    } else {
        schedule_idle_prompt(
            bridge.tts.clone(),
            Arc::clone(&bridge.accounting),
            bridge.behavior.clone(),
            Arc::clone(&record),
            speaker_tx.clone(),
            playback_ms,
            idle_generation,
        );
    }
    let total_ms = turn_started.elapsed().as_millis();
    let metrics = TurnMetrics {
        turn_started_at,
        gap_since_previous_turn_ms,
        stt_ms,
        extraction_ms,
        llm_ms,
        tts_ms,
        tts_first_audio_ms,
        total_ms,
    };
    let summary = record.record_turn_metrics(&metrics);
    info!(
        call_id = %record.call.call_id(),
        turn_index = summary.turn_index,
        gap_since_previous_turn_ms = ?metrics.gap_since_previous_turn_ms,
        stt_ms = metrics.stt_ms,
        extraction_ms = metrics.extraction_ms,
        llm_ms = metrics.llm_ms,
        tts_ms = metrics.tts_ms,
        tts_first_audio_ms = metrics.tts_first_audio_ms,
        total_turn_ms = metrics.total_ms,
        avg_total_turn_ms = summary.avg_total_ms,
        avg_stt_ms = summary.avg_stt_ms,
        avg_extraction_ms = summary.avg_extraction_ms,
        avg_llm_ms = summary.avg_llm_ms,
        avg_tts_ms = summary.avg_tts_ms,
        stt_cost_usd = format_args!("{:.8}", stt_entry.cost_usd),
        llm_cost_usd = format_args!("{:.8}", llm_cost_usd),
        tts_cost_usd = format_args!("{:.8}", tts_cost_usd),
        end_call = should_end_call,
        total_call_cost_usd = format_args!("{:.8}", record.accounting_summary().total_cost_usd),
        "assistant audio queued for RTP playback"
    );
    Ok(())
}

async fn queue_assistant_tts(
    speech: &TtsService,
    accounting: &Arc<AccountingStore>,
    record: &Arc<ManagedCall>,
    speaker_tx: &crossbeam_channel::Sender<Vec<i16>>,
    text: &str,
    operation: &'static str,
    post_tts_input_suppression_ms: u64,
) -> Result<QueuedTtsSummary> {
    queue_tts_text(
        speech,
        accounting,
        record,
        speaker_tx,
        QueuedTtsRequest {
            text,
            operation,
            post_tts_input_suppression_ms,
            voice: None,
            instructions: None,
        },
    )
    .await
}

struct QueuedTtsSummary {
    total_tts_ms: u128,
    first_audio_ms: u128,
    last_entry: ApiCallLogEntry,
    total_playback_ms: u64,
    idle_generation: u64,
    segment_count: usize,
}

struct QueuedTtsRequest<'a> {
    text: &'a str,
    operation: &'static str,
    post_tts_input_suppression_ms: u64,
    voice: Option<String>,
    instructions: Option<String>,
}

async fn queue_tts_text(
    speech: &TtsService,
    accounting: &Arc<AccountingStore>,
    record: &Arc<ManagedCall>,
    speaker_tx: &crossbeam_channel::Sender<Vec<i16>>,
    request: QueuedTtsRequest<'_>,
) -> Result<QueuedTtsSummary> {
    let segments = split_tts_text(request.text, speech.backend_name());
    let segment_count = segments.len();
    let mut total_tts_ms = 0_u128;
    let mut total_playback_ms = 0_u64;
    let mut first_audio_ms = None;
    let mut last_entry = None;

    for (index, segment) in segments.iter().enumerate() {
        let segment_started = Instant::now();
        let synthesis = speech
            .speak_text(segment, request.voice.clone(), request.instructions.clone())
            .await?;
        let segment_tts_ms = segment_started.elapsed().as_millis();
        total_tts_ms += segment_tts_ms;
        if first_audio_ms.is_none() {
            first_audio_ms = Some(total_tts_ms);
        }
        let entry = record_api_call(
            accounting,
            record,
            LoggedApiCall {
                operation: request.operation,
                endpoint: &synthesis.endpoint,
                model: &synthesis.model,
                duration_ms: segment_tts_ms,
                usage_source: synthesis.usage_source,
                estimated: synthesis.estimated,
                usage: tts_usage_for_outcome(accounting, &synthesis, segment),
            },
        )?;
        let playback_ms = pcm_playback_ms(synthesis.pcm.len());
        total_playback_ms = total_playback_ms.saturating_add(playback_ms);
        record.suppress_input_for(StdDuration::from_millis(
            playback_ms.saturating_add(request.post_tts_input_suppression_ms),
        ));
        speaker_tx
            .send(synthesis.pcm)
            .map_err(|_| anyhow!("paced pcm channel closed"))?;
        info!(
            call_id = %record.call.call_id(),
            backend = speech.backend_name(),
            segment_index = index + 1,
            segment_count,
            chars = segment.len(),
            segment_tts_ms,
            segment_playback_ms = playback_ms,
            "queued assistant TTS segment"
        );
        last_entry = Some(entry);
    }

    record.record_assistant_text(request.text.to_string());
    let idle_generation = record.note_activity();
    Ok(QueuedTtsSummary {
        total_tts_ms,
        first_audio_ms: first_audio_ms.unwrap_or(0),
        last_entry: last_entry.ok_or_else(|| anyhow!("no TTS segments were generated"))?,
        total_playback_ms,
        idle_generation,
        segment_count,
    })
}

fn tts_usage_for_outcome(
    accounting: &AccountingStore,
    synthesis: &SynthesisOutcome,
    text: &str,
) -> TokenUsage {
    if synthesis.backend == "openai" {
        return TokenUsage {
            input_text_tokens: accounting.estimate_text_tokens(&synthesis.model, text),
            cached_input_text_tokens: 0,
            output_text_tokens: 0,
            input_audio_tokens: 0,
            output_audio_tokens: accounting.estimate_output_audio_tokens(
                &synthesis.model,
                synthesis.pcm.len(),
                TELEPHONY_RATE,
            ),
        };
    }
    synthesis.usage.clone()
}

fn split_tts_text(text: &str, backend: &str) -> Vec<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return vec![String::new()];
    }
    if backend != "sherpa-onnx" {
        return vec![normalized];
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut last_break_at = None;

    for ch in normalized.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | ';' | ':') || (ch == ',' && current.len() >= 48) {
            last_break_at = Some(current.len());
        }

        if current.len() >= 24 && matches!(ch, '.' | '!' | '?' | ';' | ':') {
            let segment = current.trim();
            if !segment.is_empty() {
                segments.push(segment.to_string());
            }
            current.clear();
            last_break_at = None;
            continue;
        }

        if current.len() >= 72 {
            if let Some(index) = last_break_at.take() {
                let tail = current.split_off(index);
                let segment = current.trim();
                if !segment.is_empty() {
                    segments.push(segment.to_string());
                }
                current = tail.trim_start().to_string();
            } else if let Some(index) = current.rfind(' ') {
                let tail = current.split_off(index);
                let segment = current.trim();
                if !segment.is_empty() {
                    segments.push(segment.to_string());
                }
                current = tail.trim_start().to_string();
            }
            last_break_at = None;
        }
    }

    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }

    if segments.is_empty() {
        vec![normalized]
    } else {
        segments
    }
}

fn schedule_idle_prompt(
    speech: TtsService,
    accounting: Arc<AccountingStore>,
    behavior: BehaviorConfig,
    record: Arc<ManagedCall>,
    speaker_tx: crossbeam_channel::Sender<Vec<i16>>,
    playback_ms: u64,
    idle_generation: u64,
) {
    if behavior.idle_prompt_after_ms == 0 || behavior.idle_prompt_text.trim().is_empty() {
        return;
    }

    let call_id = record.call.call_id();
    tokio::spawn(async move {
        sleep(Duration::from_millis(
            playback_ms.saturating_add(behavior.idle_prompt_after_ms),
        ))
        .await;

        if record.end_requested.load(Ordering::SeqCst)
            || record.idle_generation() != idle_generation
        {
            return;
        }

        info!(
            call_id = %call_id,
            idle_prompt_after_ms = behavior.idle_prompt_after_ms,
            "sending idle reprompt to keep call active"
        );
        if let Err(error) = queue_assistant_tts(
            &speech,
            &accounting,
            &record,
            &speaker_tx,
            behavior.idle_prompt_text.trim(),
            "tts.idle_prompt",
            behavior.post_tts_input_suppression_ms,
        )
        .await
        {
            record.mark_error(format!("failed to send idle prompt: {error}"));
        }
    });
}

fn schedule_end_call(record: Arc<ManagedCall>, playback_ms: u64, buffer_ms: u64) {
    if record.end_requested.swap(true, Ordering::SeqCst) {
        return;
    }
    let delay_ms = playback_ms.saturating_add(buffer_ms);
    let call = Arc::clone(&record.call);
    let call_id = call.call_id();
    tokio::spawn(async move {
        info!(call_id = %call_id, delay_ms, "scheduling SIP BYE after farewell playback");
        sleep(Duration::from_millis(delay_ms)).await;
        match call.end() {
            Ok(()) => info!(call_id = %call_id, "sent SIP BYE after farewell"),
            Err(error) => {
                warn!(call_id = %call_id, error = %error, "failed to send SIP BYE after farewell")
            }
        }
    });
}

fn pcm_playback_ms(sample_count: usize) -> u64 {
    ((sample_count as u64) * 1000).div_ceil(TELEPHONY_RATE as u64)
}

struct LoggedApiCall<'a> {
    operation: &'a str,
    endpoint: &'a str,
    model: &'a str,
    duration_ms: u128,
    usage_source: &'a str,
    estimated: bool,
    usage: TokenUsage,
}

fn record_api_call(
    accounting: &AccountingStore,
    record: &ManagedCall,
    logged: LoggedApiCall<'_>,
) -> Result<ApiCallLogEntry> {
    let at = rfc3339_now();
    let entry = accounting.record_api_call(
        ApiCallContext {
            at: &at,
            call_id: &record.call.call_id(),
            direction: &record.direction,
            peer: &record.peer,
            operation: logged.operation,
            endpoint: logged.endpoint,
            model: logged.model,
            duration_ms: logged.duration_ms,
            usage_source: logged.usage_source,
            estimated: logged.estimated,
        },
        logged.usage,
    )?;
    record.record_api_call(entry.clone());
    info!(
        call_id = %record.call.call_id(),
        operation = %entry.operation,
        endpoint = %entry.endpoint,
        model = %entry.model,
        estimated = entry.estimated,
        usage_source = %entry.usage_source,
        duration_ms = entry.duration_ms,
        cost_usd = format_args!("{:.8}", entry.cost_usd),
        input_text_tokens = entry.input_text_tokens,
        cached_input_text_tokens = entry.cached_input_text_tokens,
        output_text_tokens = entry.output_text_tokens,
        input_audio_tokens = entry.input_audio_tokens,
        output_audio_tokens = entry.output_audio_tokens,
        total_tokens = entry.total_tokens,
        total_call_cost_usd = format_args!("{:.8}", record.accounting_summary().total_cost_usd),
        "recorded API accounting entry"
    );
    Ok(entry)
}

struct TurnDetector {
    buffer: Vec<i16>,
    pre_roll: VecDeque<Vec<i16>>,
    speaking: bool,
    silent_frames: usize,
    speech_frames: usize,
    silence_frames_needed: usize,
    min_frames: usize,
    pre_roll_frames: usize,
    vad_threshold: i32,
}

impl TurnDetector {
    fn new(behavior: &BehaviorConfig) -> Self {
        let frame_ms = (TELEPHONY_FRAME_SAMPLES as u64 * 1000) / TELEPHONY_RATE as u64;
        let silence_frames_needed =
            behavior.turn_silence_ms.max(frame_ms).div_ceil(frame_ms) as usize;
        let min_frames = behavior.min_utterance_ms.max(frame_ms).div_ceil(frame_ms) as usize;
        let pre_roll_frames = 250_u64.div_ceil(frame_ms) as usize;
        Self {
            buffer: Vec::new(),
            pre_roll: VecDeque::with_capacity(pre_roll_frames),
            speaking: false,
            silent_frames: 0,
            speech_frames: 0,
            silence_frames_needed,
            min_frames,
            pre_roll_frames,
            vad_threshold: i32::from(behavior.vad_threshold),
        }
    }

    fn push_frame(&mut self, frame: &[i16]) -> Option<Vec<i16>> {
        let is_speech = frame_average_amplitude(frame) >= self.vad_threshold;

        if is_speech {
            if !self.speaking {
                self.speaking = true;
                while let Some(history) = self.pre_roll.pop_front() {
                    self.buffer.extend_from_slice(&history);
                }
            }
            self.speaking = true;
            self.silent_frames = 0;
            self.speech_frames += 1;
            self.buffer.extend_from_slice(frame);
            return None;
        }

        if self.speaking {
            self.buffer.extend_from_slice(frame);
            self.silent_frames += 1;
            if self.silent_frames >= self.silence_frames_needed {
                return self.finish();
            }
        } else {
            self.remember_pre_roll(frame);
        }

        None
    }

    fn finish(&mut self) -> Option<Vec<i16>> {
        let should_emit = self.speaking && self.speech_frames >= self.min_frames;
        self.speaking = false;
        self.silent_frames = 0;
        self.speech_frames = 0;
        self.pre_roll.clear();
        if should_emit {
            Some(std::mem::take(&mut self.buffer))
        } else {
            self.buffer.clear();
            None
        }
    }

    fn reset(&mut self) {
        self.speaking = false;
        self.silent_frames = 0;
        self.speech_frames = 0;
        self.buffer.clear();
        self.pre_roll.clear();
    }

    fn remember_pre_roll(&mut self, frame: &[i16]) {
        if self.pre_roll.len() == self.pre_roll_frames {
            self.pre_roll.pop_front();
        }
        self.pre_roll.push_back(frame.to_vec());
    }
}

fn frame_average_amplitude(frame: &[i16]) -> i32 {
    if frame.is_empty() {
        return 0;
    }
    let total = frame
        .iter()
        .map(|sample| i32::from(*sample).abs())
        .sum::<i32>();
    total / frame.len() as i32
}

fn sanitize_file_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn rfc3339_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn windowed_transcript(events: &[TranscriptEvent], max_events: u32) -> Vec<TranscriptEvent> {
    let max_events = max_events as usize;
    if max_events == 0 || events.len() <= max_events {
        return events.to_vec();
    }
    events[events.len() - max_events..].to_vec()
}

fn sanitize_caller_update(
    mut update: CallerUpdate,
    latest_caller_text: &str,
) -> SanitizedCallerUpdate {
    let mut sanitized = SanitizedCallerUpdate::default();
    update.first_name = normalize_name_candidate(update.first_name);
    update.last_name = normalize_name_candidate(update.last_name);
    update.company = normalize_company_candidate(update.company);
    update.timezone = update.timezone.and_then(|value| {
        if is_valid_timezone(&value) {
            Some(value)
        } else {
            None
        }
    });

    if update.preferred_language.is_some()
        && !caller_explicitly_set_language_preference(latest_caller_text)
    {
        update.preferred_language = None;
    }

    if let Some(candidate) = update
        .email
        .take()
        .and_then(|value| normalize_email_candidate(&value))
    {
        sanitized.pending_email_confirmation = Some(candidate);
    }

    update.notes = sanitize_notes(update.notes);
    sanitized.update = update;
    sanitized
}

fn caller_explicitly_set_language_preference(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    (normalized.contains("prefer")
        || normalized.contains("speak")
        || normalized.contains("language"))
        && (normalized.contains("english")
            || normalized.contains("japanese")
            || normalized.contains("korean")
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
            || normalized.contains("thai"))
}

fn normalize_name_candidate(candidate: Option<String>) -> Option<String> {
    candidate.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.contains('@')
            || trimmed.chars().filter(|ch| ch.is_ascii_digit()).count() > 0
        {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_company_candidate(candidate: Option<String>) -> Option<String> {
    candidate.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.len() > 120 {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn sanitize_notes(notes: Vec<String>) -> Vec<String> {
    notes
        .into_iter()
        .map(|note| note.trim().to_string())
        .filter(|note| !note.is_empty() && note.len() <= 120)
        .collect()
}

fn should_extract_caller_update(
    transcript_history: &[TranscriptEvent],
    record: &ManagedCall,
    caller_text: &str,
) -> bool {
    if record.pending_email_confirmation().is_some() {
        return true;
    }

    if caller_text_indicates_profile_update(caller_text) {
        return true;
    }

    transcript_history
        .iter()
        .rev()
        .find(|event| event.role == "assistant" && event.kind == "assistant.tts")
        .map(|event| assistant_prompt_requests_profile_field(&event.text))
        .unwrap_or(false)
}

fn caller_text_indicates_profile_update(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }

    normalized.contains('@')
        || normalized.contains("email")
        || normalized.contains("phone book")
        || normalized.contains("address book")
        || normalized.contains("my record")
        || normalized.contains("profile")
        || normalized.contains("preferences")
        || normalized.contains("first name")
        || normalized.contains("last name")
        || normalized.contains("surname")
        || normalized.contains("company")
        || normalized.contains("work at")
        || normalized.contains("work for")
        || normalized.contains("calling from")
        || normalized.contains("i'm from")
        || normalized.contains("i am from")
        || normalized.contains("live in")
        || normalized.contains("based in")
        || normalized.contains("timezone")
        || normalized.contains("language")
        || normalized.contains("prefer ")
        || normalized.contains("spelt ")
        || normalized.contains("spell ")
        || normalized.contains("spelled ")
        || normalized.contains("correct")
}

fn assistant_prompt_requests_profile_field(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }

    normalized.contains("what is it")
        || normalized.contains("what's your first name")
        || normalized.contains("what is your first name")
        || normalized.contains("what's your last name")
        || normalized.contains("what is your last name")
        || normalized.contains("what's your surname")
        || normalized.contains("what is your surname")
        || normalized.contains("who should i say is calling")
        || normalized.contains("could you spell")
        || normalized.contains("can you spell")
        || normalized.contains("spell that")
        || normalized.contains("is that correct")
        || normalized.contains("let me confirm")
        || normalized.contains("just to confirm")
        || normalized.contains("confirm whether")
        || normalized.contains("where are you calling from")
        || normalized.contains("which city are you in")
        || normalized.contains("what city are you in")
}

fn reconcile_pending_email_confirmation(
    phone_book: &PhoneBookStore,
    record: &ManagedCall,
    latest_caller_text: &str,
) -> Result<()> {
    let Some(pending_email) = record.pending_email_confirmation() else {
        return Ok(());
    };

    let Some(phone_book_key) = record.phone_book_key.as_deref() else {
        record.set_pending_email_confirmation(None);
        return Ok(());
    };

    if caller_confirmed_pending_value(latest_caller_text) {
        phone_book.merge_update(
            phone_book_key,
            CallerUpdate {
                email: Some(pending_email.clone()),
                ..Default::default()
            },
        )?;
        record.set_pending_email_confirmation(None);
        info!(
            call_id = %record.call.call_id(),
            email = %pending_email,
            "confirmed pending caller email"
        );
    } else if caller_rejected_pending_value(latest_caller_text) {
        record.set_pending_email_confirmation(None);
        info!(
            call_id = %record.call.call_id(),
            email = %pending_email,
            "cleared pending caller email after rejection"
        );
    }

    Ok(())
}

fn caller_confirmed_pending_value(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized == "yes"
        || normalized == "yep"
        || normalized == "yeah"
        || normalized == "correct"
        || normalized == "that's right"
        || normalized == "that is right"
        || normalized == "that's correct"
        || normalized == "that is correct"
}

fn caller_rejected_pending_value(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized == "no"
        || normalized == "nope"
        || normalized == "wrong"
        || normalized == "incorrect"
        || normalized == "not quite"
        || normalized == "that's wrong"
        || normalized == "that is wrong"
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

fn caller_requested_immediate_hangup(text: &str) -> bool {
    let normalized = normalize_match_text(text);
    [
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

fn fast_path_end_call_response(text: &str) -> Option<String> {
    if let Some(limit) = requested_count_before_hangup(text) {
        return Some(format!("{}, bye.", count_words(limit).join(", ")));
    }
    if caller_requested_immediate_hangup(text) {
        return Some("Okay, bye.".to_string());
    }
    None
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
        return default_final_farewell().to_string();
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

fn default_final_farewell() -> &'static str {
    "Okay, no worries. See you later."
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
    if !caller_requested_immediate_hangup(&normalized) {
        return None;
    }

    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for window in tokens.windows(3) {
        if window[0] == "count"
            && window[1] == "to"
            && let Some(limit) = parse_count_word(window[2])
        {
            return Some(limit);
        }
    }
    for window in tokens.windows(2) {
        if window[0] == "count"
            && let Some(limit) = parse_count_word(window[1])
        {
            return Some(limit);
        }
    }
    None
}

fn parse_count_word(token: &str) -> Option<u8> {
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

fn count_words(limit: u8) -> Vec<&'static str> {
    let mut words = Vec::new();
    for value in 1..=limit {
        words.push(match value {
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
    words
}

fn build_initial_greeting(
    caller: Option<&crate::phonebook::CallerRecord>,
    behavior: &BehaviorConfig,
) -> String {
    let time_of_day = time_of_day_label(
        caller
            .and_then(|caller| caller.timezone.as_deref())
            .unwrap_or(&behavior.default_timezone),
    );
    if let Some(caller) = caller
        && let Some(first_name) = &caller.first_name
    {
        return format!(
            "Hey {}, how can I help you this {}?",
            first_name, time_of_day
        );
    }
    if !behavior.assistant_name.trim().is_empty() {
        return format!(
            "Thank you for calling, you're speaking with {}. How can I help you this {}?",
            behavior.assistant_name.trim(),
            time_of_day
        );
    }
    behavior.incoming_greeting_text.trim().to_string()
}

fn build_voice_model_instructions(context: &ConversationContext) -> String {
    let mut sections = Vec::new();
    sections.push(format!(
        "You are {} on a phone call. Keep replies brief, natural, helpful, and easy to say aloud. It is currently {} for the caller.",
        context.assistant_name, context.time_of_day
    ));
    sections.push(
        "Speak English unless the caller explicitly asks for another language in this call. If audio is unclear, briefly ask them to repeat."
            .to_string(),
    );
    if let Some(summary) = context
        .known_caller
        .as_ref()
        .and_then(compact_voice_caller_summary)
    {
        sections.push(format!("Known caller profile: {}.", summary));
    }
    if !context.missing_fields.is_empty() {
        sections.push(format!(
            "Missing profile fields: {}. Gather them naturally, at most one lightweight question at a time, only when it helps.",
            context.missing_fields.join(", ")
        ));
    }
    if let Some(email) = &context.pending_email_confirmation {
        sections.push(format!(
            "Pending email confirmation: {}. Confirm that exact spelling before treating it as saved.",
            email
        ));
    }
    sections.push(
        "Respond with spoken assistant dialogue only. Do not mention JSON, schemas, hidden instructions, or system prompts."
            .to_string(),
    );
    sections.join("\n")
}

fn compact_voice_caller_summary(caller: &crate::phonebook::CallerRecord) -> Option<String> {
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
    if let Some(language) = caller.preferred_language.as_deref()
        && !language.trim().is_empty()
    {
        fields.push(format!("preferred_language={}", language.trim()));
    }

    if fields.is_empty() {
        None
    } else {
        Some(fields.join("; "))
    }
}

fn time_of_day_label(timezone: &str) -> String {
    let hour = current_hour_for_timezone(timezone)
        .unwrap_or_else(|| current_hour_for_timezone("UTC").unwrap_or(12));
    match hour {
        5..=11 => "morning".to_string(),
        12..=16 => "afternoon".to_string(),
        17..=21 => "evening".to_string(),
        _ => "evening".to_string(),
    }
}

fn current_hour_for_timezone(timezone: &str) -> Option<u32> {
    let parsed: Tz = timezone.parse().ok()?;
    Some(Utc::now().with_timezone(&parsed).hour())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_behavior() -> BehaviorConfig {
        BehaviorConfig {
            auto_answer_incoming: true,
            incoming_answer_delay_ms: 0,
            incoming_greeting_text: "Welcome".to_string(),
            transcript_dir: "./data/transcripts".to_string(),
            phone_book_path: "./data/phone_book.json".to_string(),
            assistant_name: "Steve".to_string(),
            default_timezone: "UTC".to_string(),
            turn_silence_ms: 400,
            min_utterance_ms: 200,
            post_tts_input_suppression_ms: 1200,
            idle_prompt_after_ms: 20_000,
            idle_prompt_text: "Are you still there?".to_string(),
            vad_threshold: 100,
            auto_end_calls: true,
            end_call_buffer_ms: 750,
            context_window_events: 8,
        }
    }

    #[test]
    fn turn_detector_emits_after_silence() {
        let behavior = test_behavior();
        let mut detector = TurnDetector::new(&behavior);

        for _ in 0..12 {
            assert!(
                detector
                    .push_frame(&vec![500; TELEPHONY_FRAME_SAMPLES])
                    .is_none()
            );
        }

        let mut utterance = None;
        for _ in 0..20 {
            utterance = detector.push_frame(&vec![0; TELEPHONY_FRAME_SAMPLES]);
            if utterance.is_some() {
                break;
            }
        }

        let utterance = utterance.expect("utterance after silence");
        assert!(utterance.len() >= TELEPHONY_FRAME_SAMPLES * 12);
    }

    #[test]
    fn turn_detector_keeps_soft_leading_audio_before_threshold_crossing() {
        let behavior = test_behavior();
        let mut detector = TurnDetector::new(&behavior);

        for _ in 0..4 {
            assert!(
                detector
                    .push_frame(&vec![80; TELEPHONY_FRAME_SAMPLES])
                    .is_none()
            );
        }

        for _ in 0..12 {
            assert!(
                detector
                    .push_frame(&vec![500; TELEPHONY_FRAME_SAMPLES])
                    .is_none()
            );
        }

        let mut utterance = None;
        for _ in 0..20 {
            utterance = detector.push_frame(&vec![0; TELEPHONY_FRAME_SAMPLES]);
            if utterance.is_some() {
                break;
            }
        }

        let utterance = utterance.expect("utterance after silence");
        assert_eq!(utterance.first().copied(), Some(80));
        assert!(utterance.len() >= TELEPHONY_FRAME_SAMPLES * 16);
    }

    #[test]
    fn windowed_transcript_keeps_latest_events() {
        let events = (0..5)
            .map(|index| TranscriptEvent {
                role: "caller".to_string(),
                kind: "caller.transcript.completed".to_string(),
                text: index.to_string(),
                at: format!("t{index}"),
            })
            .collect::<Vec<_>>();

        let window = windowed_transcript(&events, 2);
        assert_eq!(window.len(), 2);
        assert_eq!(window[0].text, "3");
        assert_eq!(window[1].text, "4");
    }

    #[test]
    fn sanitize_caller_update_drops_inferred_language_preference() {
        let sanitized = sanitize_caller_update(
            crate::phonebook::CallerUpdate {
                preferred_language: Some("Japanese".to_string()),
                ..Default::default()
            },
            "すごい。",
        );

        assert_eq!(sanitized.update.preferred_language, None);
    }

    #[test]
    fn sanitize_caller_update_keeps_explicit_language_preference() {
        let sanitized = sanitize_caller_update(
            crate::phonebook::CallerUpdate {
                preferred_language: Some("Japanese".to_string()),
                ..Default::default()
            },
            "I prefer Japanese when we speak.",
        );

        assert_eq!(
            sanitized.update.preferred_language.as_deref(),
            Some("Japanese")
        );
    }

    #[test]
    fn sanitize_caller_update_requires_email_confirmation() {
        let sanitized = sanitize_caller_update(
            crate::phonebook::CallerUpdate {
                email: Some("David@example.com".to_string()),
                ..Default::default()
            },
            "My email is david@example.com",
        );

        assert_eq!(sanitized.update.email, None);
        assert_eq!(
            sanitized.pending_email_confirmation.as_deref(),
            Some("david@example.com")
        );
    }

    #[test]
    fn caller_text_indicates_profile_update_detects_contact_fields() {
        assert!(caller_text_indicates_profile_update(
            "My email is david@example.com"
        ));
        assert!(caller_text_indicates_profile_update(
            "My last name is Hooton"
        ));
        assert!(caller_text_indicates_profile_update(
            "I work at Example Corp"
        ));
        assert!(!caller_text_indicates_profile_update(
            "Can you tell me a joke?"
        ));
    }

    #[test]
    fn assistant_prompt_requests_profile_field_detects_follow_up_prompts() {
        assert!(assistant_prompt_requests_profile_field(
            "I don't have your last name yet. What is it?"
        ));
        assert!(assistant_prompt_requests_profile_field(
            "Could you spell that email address for me?"
        ));
        assert!(!assistant_prompt_requests_profile_field(
            "The fields I can edit include your first name, last name, email, company, timezone, and preferred language. Is there something specific you need help with?"
        ));
        assert!(!assistant_prompt_requests_profile_field(
            "Anything else you need help with tonight?"
        ));
    }

    #[test]
    fn split_tts_text_prefers_sentence_boundaries_for_local_tts() {
        let segments = split_tts_text(
            "One, two, three, four, five, six, seven. Just hanging out with you! What else can I do for you tonight?",
            "sherpa-onnx",
        );

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], "One, two, three, four, five, six, seven.");
        assert_eq!(segments[1], "Just hanging out with you!");
        assert_eq!(segments[2], "What else can I do for you tonight?");
    }

    #[test]
    fn split_tts_text_keeps_openai_text_as_single_segment() {
        let segments = split_tts_text(
            "This is a long enough response that local TTS would segment it, but OpenAI should keep it together as one request.",
            "openai",
        );

        assert_eq!(segments, vec!["This is a long enough response that local TTS would segment it, but OpenAI should keep it together as one request.".to_string()]);
    }

    #[test]
    fn caller_requested_end_call_detects_farewell_and_garbled_hangup() {
        assert!(caller_requested_end_call("Goodbye for now."));
        assert!(caller_requested_end_call("It's re-inhanging up."));
        assert!(caller_requested_end_call(
            "Please count to three and hang up."
        ));
        assert!(!caller_requested_end_call(
            "Can you help me update my email?"
        ));
    }

    #[test]
    fn finalize_end_call_response_appends_a_real_closing() {
        assert_eq!(
            finalize_end_call_response("One, two, three."),
            "One, two, three. See you later."
        );
        assert_eq!(
            finalize_end_call_response("I didn't catch that, could you repeat it?"),
            default_final_farewell()
        );
    }

    #[test]
    fn looks_like_final_farewell_requires_an_actual_closing() {
        assert!(looks_like_final_farewell("Okay, see you later."));
        assert!(!looks_like_final_farewell(
            "I didn't catch that, could you repeat it?"
        ));
    }

    #[test]
    fn fast_path_end_call_response_counts_then_hangs_up_without_llm() {
        assert_eq!(
            fast_path_end_call_response("Count three and hang up.").as_deref(),
            Some("one, two, three, bye.")
        );
        assert_eq!(
            fast_path_end_call_response("Count to 3 and hang up.").as_deref(),
            Some("one, two, three, bye.")
        );
    }

    #[test]
    fn fast_path_end_call_response_shortens_immediate_hangup_requests() {
        assert_eq!(
            fast_path_end_call_response("Hang up now.").as_deref(),
            Some("Okay, bye.")
        );
        assert_eq!(fast_path_end_call_response("Goodbye."), None);
    }
}
