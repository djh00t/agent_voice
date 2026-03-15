//! Runtime configuration loading for SIP, OpenAI, behavior, and accounting.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use xphone::{Codec, Config as PhoneConfig};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
/// The top-level application configuration.
pub struct AppConfig {
    pub sip: SipConfig,
    pub openai: OpenAiConfig,
    pub agent_api: AgentApiConfig,
    #[serde(default)]
    pub behavior: BehaviorConfig,
    #[serde(default)]
    pub accounting: AccountingConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

impl AppConfig {
    /// Loads configuration from an optional YAML file plus environment overrides.
    pub fn load(path: Option<&Path>, require_path: bool) -> Result<Self> {
        let mut config = match path {
            Some(path) => {
                let raw = fs::read_to_string(path)
                    .with_context(|| format!("failed to read config file {}", path.display()))?;
                serde_yaml::from_str(&raw).with_context(|| "failed to parse YAML config")?
            }
            None if require_path => {
                bail!("configuration file path was required but not provided");
            }
            None => Self::default(),
        };
        config.apply_env_overrides_from_map(&std::env::vars().collect());
        config.validate()?;
        Ok(config)
    }

    /// Returns the first conventional config file path that exists on disk.
    pub fn resolve_default_path() -> Option<PathBuf> {
        [
            PathBuf::from("./config/agent_voice.yaml"),
            PathBuf::from("/opt/agent_voice/config/agent_voice.yaml"),
        ]
        .into_iter()
        .find(|candidate| candidate.exists())
    }

    fn apply_env_overrides_from_map(&mut self, env: &std::collections::HashMap<String, String>) {
        apply_string(env, "SIP_USERNAME", &mut self.sip.username);
        apply_string(env, "SIP_PASSWORD", &mut self.sip.password);
        apply_string(env, "SIP_HOST", &mut self.sip.host);
        apply_u16(env, "SIP_PORT", &mut self.sip.port);
        apply_string(env, "SIP_TRANSPORT", &mut self.sip.transport);
        apply_optional_string(env, "SIP_LOCAL_IP", &mut self.sip.local_ip);
        apply_u16(env, "SIP_RTP_PORT_MIN", &mut self.sip.rtp_port_min);
        apply_u16(env, "SIP_RTP_PORT_MAX", &mut self.sip.rtp_port_max);
        apply_u64(
            env,
            "SIP_REGISTER_EXPIRY_SECS",
            &mut self.sip.register_expiry_secs,
        );
        apply_u64(
            env,
            "SIP_REGISTER_RETRY_SECS",
            &mut self.sip.register_retry_secs,
        );
        apply_u32(
            env,
            "SIP_REGISTER_MAX_RETRY",
            &mut self.sip.register_max_retry,
        );
        apply_optional_u64(
            env,
            "SIP_NAT_KEEPALIVE_SECS",
            &mut self.sip.nat_keepalive_secs,
        );
        apply_optional_string(env, "SIP_STUN_SERVER", &mut self.sip.stun_server);
        apply_bool(
            env,
            "SIP_ACCEPT_INCOMING_CALLS",
            &mut self.sip.accept_incoming_calls,
        );
        apply_string_list(env, "SIP_PREFERRED_CODECS", &mut self.sip.preferred_codecs);

        apply_optional_string(env, "OPENAI_API_KEY", &mut self.openai.api_key);
        apply_string(env, "OPENAI_REALTIME_URL", &mut self.openai.realtime_url);
        apply_string(env, "OPENAI_AUDIO_API_URL", &mut self.openai.audio_api_url);
        apply_string(
            env,
            "OPENAI_TRANSCRIPTION_MODEL",
            &mut self.openai.transcription_model,
        );
        apply_string(
            env,
            "OPENAI_TRANSCRIPTION_API_URL",
            &mut self.openai.transcription_api_url,
        );
        apply_string(env, "OPENAI_TTS_MODEL", &mut self.openai.tts_model);
        apply_string(env, "OPENAI_TTS_VOICE", &mut self.openai.tts_voice);
        apply_optional_string(
            env,
            "OPENAI_TTS_INSTRUCTIONS",
            &mut self.openai.tts_instructions,
        );
        apply_string(
            env,
            "OPENAI_RESPONSES_API_URL",
            &mut self.openai.responses_api_url,
        );
        apply_string(
            env,
            "OPENAI_RESPONSE_MODEL",
            &mut self.openai.response_model,
        );
        apply_optional_string(
            env,
            "OPENAI_TRANSCRIPTION_PROMPT",
            &mut self.openai.transcription_prompt,
        );
        apply_optional_string(
            env,
            "OPENAI_TRANSCRIPTION_LANGUAGE",
            &mut self.openai.transcription_language,
        );
        apply_string(env, "OPENAI_TTS_FORMAT", &mut self.openai.tts_format);
        apply_optional_string(
            env,
            "OPENAI_RESPONSE_INSTRUCTIONS",
            &mut self.openai.response_instructions,
        );

        apply_string(env, "AGENT_API_LISTEN", &mut self.agent_api.listen);
        apply_bool(
            env,
            "AUTO_ANSWER_INCOMING",
            &mut self.behavior.auto_answer_incoming,
        );
        apply_u64(
            env,
            "INCOMING_ANSWER_DELAY_MS",
            &mut self.behavior.incoming_answer_delay_ms,
        );
        apply_string(
            env,
            "INCOMING_GREETING_TEXT",
            &mut self.behavior.incoming_greeting_text,
        );
        apply_string(env, "TRANSCRIPT_DIR", &mut self.behavior.transcript_dir);
        apply_string(env, "PHONE_BOOK_PATH", &mut self.behavior.phone_book_path);
        apply_string(env, "ASSISTANT_NAME", &mut self.behavior.assistant_name);
        apply_string(env, "DEFAULT_TIMEZONE", &mut self.behavior.default_timezone);
        apply_u64(
            env,
            "CALL_TURN_SILENCE_MS",
            &mut self.behavior.turn_silence_ms,
        );
        apply_u64(
            env,
            "CALL_MIN_UTTERANCE_MS",
            &mut self.behavior.min_utterance_ms,
        );
        apply_u64(
            env,
            "POST_TTS_INPUT_SUPPRESSION_MS",
            &mut self.behavior.post_tts_input_suppression_ms,
        );
        apply_u16(env, "CALL_VAD_THRESHOLD", &mut self.behavior.vad_threshold);
        apply_bool(env, "AUTO_END_CALLS", &mut self.behavior.auto_end_calls);
        apply_u64(
            env,
            "END_CALL_BUFFER_MS",
            &mut self.behavior.end_call_buffer_ms,
        );
        apply_u32(
            env,
            "CALL_CONTEXT_WINDOW_EVENTS",
            &mut self.behavior.context_window_events,
        );
        apply_string(
            env,
            "ACCOUNTING_MODEL_CATALOG_PATH",
            &mut self.accounting.model_catalog_path,
        );
        apply_string(
            env,
            "ACCOUNTING_API_CALLS_CSV_PATH",
            &mut self.accounting.api_calls_csv_path,
        );
        apply_string(
            env,
            "ACCOUNTING_CALL_TOTALS_CSV_PATH",
            &mut self.accounting.call_totals_csv_path,
        );
        apply_string(
            env,
            "ACCOUNTING_PRICING_PAGE_URL",
            &mut self.accounting.pricing_page_url,
        );
        apply_bool(
            env,
            "ACCOUNTING_REFRESH_PRICING_ON_STARTUP",
            &mut self.accounting.refresh_pricing_on_startup,
        );
        apply_string(env, "AGENT_VOICE_LOG_LEVEL", &mut self.logging.level);
    }

    fn validate(&self) -> Result<()> {
        if self
            .openai
            .api_key
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        {
            bail!("OpenAI API key is required via config or OPENAI_API_KEY");
        }
        if self.sip.username.is_empty() {
            bail!("sip.username must not be empty");
        }
        if self.sip.password.is_empty() {
            bail!("sip.password must not be empty");
        }
        if self.sip.host.is_empty() {
            bail!("sip.host must not be empty");
        }
        if self.agent_api.listen.is_empty() {
            bail!("agent_api.listen must not be empty");
        }
        Ok(())
    }

    /// Converts the app config into an `xphone` SIP phone configuration.
    pub fn phone_config(&self) -> PhoneConfig {
        let mut config = PhoneConfig {
            username: self.sip.username.clone(),
            password: self.sip.password.clone(),
            host: self.sip.host.clone(),
            port: self.sip.port,
            transport: self.sip.transport.clone(),
            local_ip: self.sip.local_ip.clone().unwrap_or_default(),
            rtp_port_min: self.sip.rtp_port_min,
            rtp_port_max: self.sip.rtp_port_max,
            codec_prefs: self.sip.codec_preferences(),
            register_expiry: Duration::from_secs(self.sip.register_expiry_secs),
            register_retry: Duration::from_secs(self.sip.register_retry_secs),
            register_max_retry: self.sip.register_max_retry,
            nat_keepalive_interval: self.sip.nat_keepalive_secs.map(Duration::from_secs),
            pcm_rate: 8_000,
            pcm_frame_size: 160,
            ..PhoneConfig::default()
        };
        if let Some(stun_server) = &self.sip.stun_server {
            config.stun_server = Some(stun_server.clone());
        }
        config
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// SIP registration, transport, and media settings.
pub struct SipConfig {
    pub username: String,
    pub password: String,
    pub host: String,
    #[serde(default = "default_sip_port")]
    pub port: u16,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub local_ip: Option<String>,
    #[serde(default = "default_rtp_port_min")]
    pub rtp_port_min: u16,
    #[serde(default = "default_rtp_port_max")]
    pub rtp_port_max: u16,
    #[serde(default = "default_register_expiry_secs")]
    pub register_expiry_secs: u64,
    #[serde(default = "default_register_retry_secs")]
    pub register_retry_secs: u64,
    #[serde(default = "default_register_max_retry")]
    pub register_max_retry: u32,
    #[serde(default)]
    pub nat_keepalive_secs: Option<u64>,
    #[serde(default)]
    pub stun_server: Option<String>,
    #[serde(default)]
    pub accept_incoming_calls: bool,
    #[serde(default = "default_preferred_codecs")]
    pub preferred_codecs: Vec<String>,
}

impl SipConfig {
    fn codec_preferences(&self) -> Vec<Codec> {
        let mut codecs = Vec::new();
        for codec in &self.preferred_codecs {
            let parsed = match codec.to_ascii_uppercase().as_str() {
                "PCMU" => Some(Codec::PCMU),
                "PCMA" => Some(Codec::PCMA),
                "G722" => Some(Codec::G722),
                "G729" => Some(Codec::G729),
                "OPUS" => Some(Codec::Opus),
                _ => None,
            };
            if let Some(codec) = parsed {
                codecs.push(codec);
            }
        }
        if codecs.is_empty() {
            vec![Codec::PCMU, Codec::PCMA]
        } else {
            codecs
        }
    }
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            host: String::new(),
            port: default_sip_port(),
            transport: default_transport(),
            local_ip: None,
            rtp_port_min: default_rtp_port_min(),
            rtp_port_max: default_rtp_port_max(),
            register_expiry_secs: default_register_expiry_secs(),
            register_retry_secs: default_register_retry_secs(),
            register_max_retry: default_register_max_retry(),
            nat_keepalive_secs: None,
            stun_server: None,
            accept_incoming_calls: true,
            preferred_codecs: default_preferred_codecs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// OpenAI endpoints, models, and prompt defaults.
pub struct OpenAiConfig {
    pub api_key: Option<String>,
    #[serde(default = "default_realtime_url")]
    pub realtime_url: String,
    #[serde(default = "default_audio_api_url")]
    pub audio_api_url: String,
    #[serde(default = "default_transcription_model")]
    pub transcription_model: String,
    #[serde(default = "default_transcription_api_url")]
    pub transcription_api_url: String,
    #[serde(default = "default_tts_model")]
    pub tts_model: String,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    #[serde(default = "default_tts_instructions")]
    pub tts_instructions: Option<String>,
    #[serde(default = "default_responses_api_url")]
    pub responses_api_url: String,
    #[serde(default = "default_response_model")]
    pub response_model: String,
    #[serde(default)]
    pub transcription_prompt: Option<String>,
    #[serde(default)]
    pub transcription_language: Option<String>,
    #[serde(default = "default_tts_format")]
    pub tts_format: String,
    #[serde(default = "default_response_instructions")]
    pub response_instructions: Option<String>,
}

impl OpenAiConfig {
    /// Returns the configured API key or an empty string when unset.
    pub fn api_key(&self) -> &str {
        self.api_key.as_deref().unwrap_or_default()
    }
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            realtime_url: default_realtime_url(),
            audio_api_url: default_audio_api_url(),
            transcription_model: default_transcription_model(),
            transcription_api_url: default_transcription_api_url(),
            tts_model: default_tts_model(),
            tts_voice: default_tts_voice(),
            tts_instructions: default_tts_instructions(),
            responses_api_url: default_responses_api_url(),
            response_model: default_response_model(),
            transcription_prompt: None,
            transcription_language: None,
            tts_format: default_tts_format(),
            response_instructions: default_response_instructions(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// HTTP listener settings for the local agent control API.
pub struct AgentApiConfig {
    pub listen: String,
}

impl Default for AgentApiConfig {
    fn default() -> Self {
        Self {
            listen: default_agent_api_listen(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Model catalog and CSV accounting output paths.
pub struct AccountingConfig {
    #[serde(default = "default_model_catalog_path")]
    pub model_catalog_path: String,
    #[serde(default = "default_api_calls_csv_path")]
    pub api_calls_csv_path: String,
    #[serde(default = "default_call_totals_csv_path")]
    pub call_totals_csv_path: String,
    #[serde(default = "default_pricing_page_url")]
    pub pricing_page_url: String,
    #[serde(default = "default_refresh_pricing_on_startup")]
    pub refresh_pricing_on_startup: bool,
}

impl Default for AccountingConfig {
    fn default() -> Self {
        Self {
            model_catalog_path: default_model_catalog_path(),
            api_calls_csv_path: default_api_calls_csv_path(),
            call_totals_csv_path: default_call_totals_csv_path(),
            pricing_page_url: default_pricing_page_url(),
            refresh_pricing_on_startup: default_refresh_pricing_on_startup(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Runtime conversation and call-behavior tuning.
pub struct BehaviorConfig {
    #[serde(default = "default_auto_answer")]
    pub auto_answer_incoming: bool,
    #[serde(default)]
    pub incoming_answer_delay_ms: u64,
    #[serde(default = "default_incoming_greeting_text")]
    pub incoming_greeting_text: String,
    #[serde(default = "default_transcript_dir")]
    pub transcript_dir: String,
    #[serde(default = "default_phone_book_path")]
    pub phone_book_path: String,
    #[serde(default = "default_assistant_name")]
    pub assistant_name: String,
    #[serde(default = "default_timezone")]
    pub default_timezone: String,
    #[serde(default = "default_turn_silence_ms")]
    pub turn_silence_ms: u64,
    #[serde(default = "default_min_utterance_ms")]
    pub min_utterance_ms: u64,
    #[serde(default = "default_post_tts_input_suppression_ms")]
    pub post_tts_input_suppression_ms: u64,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: u16,
    #[serde(default = "default_auto_end_calls")]
    pub auto_end_calls: bool,
    #[serde(default = "default_end_call_buffer_ms")]
    pub end_call_buffer_ms: u64,
    #[serde(default = "default_context_window_events")]
    pub context_window_events: u32,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            auto_answer_incoming: default_auto_answer(),
            incoming_answer_delay_ms: 0,
            incoming_greeting_text: default_incoming_greeting_text(),
            transcript_dir: default_transcript_dir(),
            phone_book_path: default_phone_book_path(),
            assistant_name: default_assistant_name(),
            default_timezone: default_timezone(),
            turn_silence_ms: default_turn_silence_ms(),
            min_utterance_ms: default_min_utterance_ms(),
            post_tts_input_suppression_ms: default_post_tts_input_suppression_ms(),
            vad_threshold: default_vad_threshold(),
            auto_end_calls: default_auto_end_calls(),
            end_call_buffer_ms: default_end_call_buffer_ms(),
            context_window_events: default_context_window_events(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Logging configuration for the service.
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

const fn default_sip_port() -> u16 {
    5060
}

fn default_transport() -> String {
    "udp".to_string()
}

const fn default_rtp_port_min() -> u16 {
    10_000
}

const fn default_rtp_port_max() -> u16 {
    20_000
}

const fn default_register_expiry_secs() -> u64 {
    300
}

const fn default_register_retry_secs() -> u64 {
    2
}

const fn default_register_max_retry() -> u32 {
    5
}

fn default_preferred_codecs() -> Vec<String> {
    vec!["PCMU".to_string(), "PCMA".to_string()]
}

fn default_realtime_url() -> String {
    "wss://api.openai.com/v1/realtime".to_string()
}

fn default_audio_api_url() -> String {
    "https://api.openai.com/v1/audio/speech".to_string()
}

fn default_transcription_api_url() -> String {
    "https://api.openai.com/v1/audio/transcriptions".to_string()
}

fn default_transcription_model() -> String {
    "gpt-4o-transcribe".to_string()
}

fn default_tts_model() -> String {
    "gpt-4o-mini-tts".to_string()
}

fn default_tts_voice() -> String {
    "alloy".to_string()
}

fn default_tts_instructions() -> Option<String> {
    Some(
        "Speak in a cheerful, upbeat, warm, and helpful tone with a friendly Australian accent. Sound engaged and natural, not flat, stiff, or monotone."
            .to_string(),
    )
}

fn default_responses_api_url() -> String {
    "https://api.openai.com/v1/responses".to_string()
}

fn default_response_model() -> String {
    "gpt-4o-mini".to_string()
}

fn default_tts_format() -> String {
    "wav".to_string()
}

const fn default_auto_answer() -> bool {
    true
}

fn default_log_level() -> String {
    "info,agent_voice=debug".to_string()
}

fn default_agent_api_listen() -> String {
    "127.0.0.1:8089".to_string()
}

fn default_model_catalog_path() -> String {
    "./accounting/models.json".to_string()
}

fn default_api_calls_csv_path() -> String {
    "./accounting/api_calls.csv".to_string()
}

fn default_call_totals_csv_path() -> String {
    "./accounting/call_totals.csv".to_string()
}

fn default_pricing_page_url() -> String {
    "https://developers.openai.com/api/docs/pricing".to_string()
}

const fn default_refresh_pricing_on_startup() -> bool {
    true
}

fn default_incoming_greeting_text() -> String {
    "Welcome".to_string()
}

fn default_transcript_dir() -> String {
    "./data/transcripts".to_string()
}

fn default_phone_book_path() -> String {
    "./data/phone_book.json".to_string()
}

fn default_assistant_name() -> String {
    "Steve".to_string()
}

fn default_timezone() -> String {
    "UTC".to_string()
}

fn default_response_instructions() -> Option<String> {
    Some(
        "You are a helpful voice agent on a phone call. Keep replies brief, natural, and conversational."
            .to_string(),
    )
}

const fn default_turn_silence_ms() -> u64 {
    1200
}

const fn default_min_utterance_ms() -> u64 {
    400
}

const fn default_post_tts_input_suppression_ms() -> u64 {
    1200
}

const fn default_vad_threshold() -> u16 {
    500
}

const fn default_auto_end_calls() -> bool {
    true
}

const fn default_end_call_buffer_ms() -> u64 {
    750
}

const fn default_context_window_events() -> u32 {
    8
}

fn apply_string(env: &std::collections::HashMap<String, String>, key: &str, target: &mut String) {
    if let Some(value) = env.get(key).map(String::as_str).map(normalize_env_value)
        && !value.is_empty()
    {
        *target = value.to_string();
    }
}

fn apply_optional_string(
    env: &std::collections::HashMap<String, String>,
    key: &str,
    target: &mut Option<String>,
) {
    if let Some(value) = env.get(key) {
        let trimmed = normalize_env_value(value);
        *target = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
}

fn apply_u16(env: &std::collections::HashMap<String, String>, key: &str, target: &mut u16) {
    if let Some(value) = parse_number::<u16>(env, key) {
        *target = value;
    }
}

fn apply_u32(env: &std::collections::HashMap<String, String>, key: &str, target: &mut u32) {
    if let Some(value) = parse_number::<u32>(env, key) {
        *target = value;
    }
}

fn apply_u64(env: &std::collections::HashMap<String, String>, key: &str, target: &mut u64) {
    if let Some(value) = parse_number::<u64>(env, key) {
        *target = value;
    }
}

fn apply_optional_u64(
    env: &std::collections::HashMap<String, String>,
    key: &str,
    target: &mut Option<u64>,
) {
    if let Some(value) = env.get(key) {
        let trimmed = normalize_env_value(value);
        *target = if trimmed.is_empty() {
            None
        } else {
            trimmed.parse::<u64>().ok()
        };
    }
}

fn apply_bool(env: &std::collections::HashMap<String, String>, key: &str, target: &mut bool) {
    if let Some(value) = env.get(key).and_then(|value| parse_bool(value)) {
        *target = value;
    }
}

fn apply_string_list(
    env: &std::collections::HashMap<String, String>,
    key: &str,
    target: &mut Vec<String>,
) {
    if let Some(value) = env.get(key) {
        let list = normalize_env_value(value)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !list.is_empty() {
            *target = list;
        }
    }
}

fn parse_number<T: std::str::FromStr>(
    env: &std::collections::HashMap<String, String>,
    key: &str,
) -> Option<T> {
    env.get(key)
        .map(String::as_str)
        .map(normalize_env_value)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<T>().ok())
}

fn parse_bool(value: &str) -> Option<bool> {
    match normalize_env_value(value).to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn normalize_env_value(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn defaults_map_to_supported_codecs() {
        let sip = SipConfig {
            username: "u".into(),
            password: "p".into(),
            host: "example.com".into(),
            port: default_sip_port(),
            transport: default_transport(),
            local_ip: None,
            rtp_port_min: default_rtp_port_min(),
            rtp_port_max: default_rtp_port_max(),
            register_expiry_secs: default_register_expiry_secs(),
            register_retry_secs: default_register_retry_secs(),
            register_max_retry: default_register_max_retry(),
            nat_keepalive_secs: None,
            stun_server: None,
            accept_incoming_calls: true,
            preferred_codecs: default_preferred_codecs(),
        };

        assert_eq!(sip.codec_preferences(), vec![Codec::PCMU, Codec::PCMA]);
    }

    #[test]
    fn env_map_overrides_config() {
        let mut config = AppConfig::default();
        let env = HashMap::from([
            ("SIP_USERNAME".to_string(), "alice".to_string()),
            ("SIP_PASSWORD".to_string(), "secret".to_string()),
            ("SIP_HOST".to_string(), "sip.example.com".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-test".to_string()),
            (
                "OPENAI_RESPONSE_MODEL".to_string(),
                "gpt-4o-mini".to_string(),
            ),
            ("AGENT_API_LISTEN".to_string(), "0.0.0.0:8089".to_string()),
            ("SIP_PREFERRED_CODECS".to_string(), "PCMU,PCMA".to_string()),
            ("AUTO_ANSWER_INCOMING".to_string(), "false".to_string()),
            ("INCOMING_ANSWER_DELAY_MS".to_string(), "2000".to_string()),
            ("INCOMING_GREETING_TEXT".to_string(), "Welcome".to_string()),
            (
                "PHONE_BOOK_PATH".to_string(),
                "./data/phone_book.json".to_string(),
            ),
            ("ASSISTANT_NAME".to_string(), "Steve".to_string()),
            (
                "DEFAULT_TIMEZONE".to_string(),
                "Australia/Sydney".to_string(),
            ),
            ("CALL_TURN_SILENCE_MS".to_string(), "1500".to_string()),
            ("CALL_MIN_UTTERANCE_MS".to_string(), "500".to_string()),
            (
                "POST_TTS_INPUT_SUPPRESSION_MS".to_string(),
                "900".to_string(),
            ),
            ("CALL_VAD_THRESHOLD".to_string(), "600".to_string()),
            ("AUTO_END_CALLS".to_string(), "false".to_string()),
            ("END_CALL_BUFFER_MS".to_string(), "1200".to_string()),
            ("CALL_CONTEXT_WINDOW_EVENTS".to_string(), "10".to_string()),
            (
                "ACCOUNTING_MODEL_CATALOG_PATH".to_string(),
                "./accounting/models.json".to_string(),
            ),
            (
                "ACCOUNTING_API_CALLS_CSV_PATH".to_string(),
                "./accounting/api_calls.csv".to_string(),
            ),
            (
                "ACCOUNTING_CALL_TOTALS_CSV_PATH".to_string(),
                "./accounting/call_totals.csv".to_string(),
            ),
            (
                "ACCOUNTING_PRICING_PAGE_URL".to_string(),
                "https://developers.openai.com/api/docs/pricing".to_string(),
            ),
            (
                "ACCOUNTING_REFRESH_PRICING_ON_STARTUP".to_string(),
                "false".to_string(),
            ),
            (
                "TRANSCRIPT_DIR".to_string(),
                "./data/transcripts".to_string(),
            ),
        ]);

        config.apply_env_overrides_from_map(&env);

        assert_eq!(config.sip.username, "alice");
        assert_eq!(config.sip.password, "secret");
        assert_eq!(config.sip.host, "sip.example.com");
        assert_eq!(config.openai.api_key(), "sk-test");
        assert_eq!(config.openai.response_model, "gpt-4o-mini");
        assert_eq!(config.agent_api.listen, "0.0.0.0:8089");
        assert_eq!(config.sip.preferred_codecs, vec!["PCMU", "PCMA"]);
        assert!(!config.behavior.auto_answer_incoming);
        assert_eq!(config.behavior.incoming_answer_delay_ms, 2000);
        assert_eq!(config.behavior.incoming_greeting_text, "Welcome");
        assert_eq!(config.behavior.transcript_dir, "./data/transcripts");
        assert_eq!(config.behavior.phone_book_path, "./data/phone_book.json");
        assert_eq!(config.behavior.assistant_name, "Steve");
        assert_eq!(config.behavior.default_timezone, "Australia/Sydney");
        assert_eq!(config.behavior.turn_silence_ms, 1500);
        assert_eq!(config.behavior.min_utterance_ms, 500);
        assert_eq!(config.behavior.post_tts_input_suppression_ms, 900);
        assert_eq!(config.behavior.vad_threshold, 600);
        assert!(!config.behavior.auto_end_calls);
        assert_eq!(config.behavior.end_call_buffer_ms, 1200);
        assert_eq!(config.behavior.context_window_events, 10);
        assert_eq!(
            config.accounting.model_catalog_path,
            "./accounting/models.json"
        );
        assert_eq!(
            config.accounting.api_calls_csv_path,
            "./accounting/api_calls.csv"
        );
        assert_eq!(
            config.accounting.call_totals_csv_path,
            "./accounting/call_totals.csv"
        );
        assert_eq!(
            config.accounting.pricing_page_url,
            "https://developers.openai.com/api/docs/pricing"
        );
        assert!(!config.accounting.refresh_pricing_on_startup);
    }

    #[test]
    fn env_map_strips_surrounding_quotes() {
        let mut config = AppConfig::default();
        let env = HashMap::from([
            ("SIP_USERNAME".to_string(), "\"alice\"".to_string()),
            ("OPENAI_API_KEY".to_string(), "'sk-test'".to_string()),
            ("AUTO_ANSWER_INCOMING".to_string(), "\"false\"".to_string()),
            ("INCOMING_ANSWER_DELAY_MS".to_string(), "'2000'".to_string()),
        ]);

        config.apply_env_overrides_from_map(&env);

        assert_eq!(config.sip.username, "alice");
        assert_eq!(config.openai.api_key(), "sk-test");
        assert!(!config.behavior.auto_answer_incoming);
        assert_eq!(config.behavior.incoming_answer_delay_ms, 2000);
    }

    #[test]
    fn parse_bool_accepts_common_forms() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }
}
