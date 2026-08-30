//! Bounded, provider-independent values used by the Realtime boundary.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum bytes accepted for one provider event.
pub const MAX_EVENT_BYTES: usize = 65_536;
/// Maximum bytes accepted for one opaque identifier.
pub const MAX_ID_BYTES: usize = 128;
/// Maximum characters emitted by a value error.
pub const MAX_ERROR_MESSAGE_CHARS: usize = 512;

const INVALID_OPAQUE_ID: &str = "realtime_invalid_opaque_id";
const INVALID_TRANSCRIPT_TEXT: &str = "realtime_invalid_transcript_text";
const INVALID_FUNCTION_ARGUMENTS: &str = "realtime_invalid_function_arguments";
const INVALID_TOOL_OUTPUT: &str = "realtime_invalid_tool_output";
const INVALID_BASE64: &str = "realtime_invalid_base64";

const MAX_TRANSCRIPT_SCALARS: usize = 4_096;
const MAX_VALUE_BYTES: usize = 16_384;
const MAX_AUDIO_BASE64_CHARS: usize = 21_848;

/// Redacted failures returned while decoding bounded Realtime values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeValueError {
    /// The event exceeded [`MAX_EVENT_BYTES`].
    EventTooLarge,
    /// The event was not valid JSON.
    InvalidJson,
    /// The event type was not in the closed event set.
    UnknownEventType,
    /// A required field was absent or null.
    MissingField(&'static str),
    /// An opaque identifier failed validation.
    InvalidOpaqueId,
    /// Audio was empty.
    EmptyAudio,
    /// Audio was not valid base64.
    InvalidBase64,
    /// Audio exceeded its bound.
    AudioTooLarge,
    /// Transcript text exceeded its bound.
    TranscriptTooLong,
    /// Function arguments exceeded their bound.
    ArgumentsTooLong,
    /// Function arguments were not valid JSON.
    InvalidArgumentsJson,
    /// Tool output exceeded its bound.
    ToolOutputTooLong,
    /// The audio format was not supported.
    UnsupportedAudioFormat,
    /// A response status was invalid.
    InvalidResponseStatus,
    /// An interruption reason was invalid.
    InvalidInterruptionReason,
}

impl fmt::Display for RealtimeValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EventTooLarge => "event is too large",
            Self::InvalidJson => "invalid JSON",
            Self::UnknownEventType => "unknown event type",
            Self::MissingField(_) => "missing required field",
            Self::InvalidOpaqueId => "invalid opaque identifier",
            Self::EmptyAudio => "audio is empty",
            Self::InvalidBase64 => "invalid base64 audio",
            Self::AudioTooLarge => "audio is too large",
            Self::TranscriptTooLong => "transcript is too long",
            Self::ArgumentsTooLong => "function arguments are too long",
            Self::InvalidArgumentsJson => "function arguments are not a JSON object",
            Self::ToolOutputTooLong => "tool output is too long",
            Self::UnsupportedAudioFormat => "unsupported audio format",
            Self::InvalidResponseStatus => "invalid response status",
            Self::InvalidInterruptionReason => "invalid interruption reason",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RealtimeValueError {}

/// An opaque provider identifier with a bounded, ASCII-safe alphabet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueId(String);

impl OpaqueId {
    /// Validates and constructs an opaque identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_ID_BYTES
            || !value.is_ascii()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
        {
            return Err(RealtimeValueError::InvalidOpaqueId);
        }
        Ok(Self(value))
    }

    /// Returns the validated identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the validated identifier, consuming the wrapper.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for OpaqueId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl TryFrom<&str> for OpaqueId {
    type Error = RealtimeValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for OpaqueId {
    type Error = RealtimeValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for OpaqueId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OpaqueId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value =
            String::deserialize(deserializer).map_err(|_| D::Error::custom(INVALID_OPAQUE_ID))?;
        Self::new(value).map_err(|_| D::Error::custom(INVALID_OPAQUE_ID))
    }
}

/// Lossless transcript text bounded by Unicode scalar values.
#[derive(Clone, PartialEq, Eq)]
pub struct TranscriptText(String);

impl TranscriptText {
    /// Constructs transcript text with at most 4096 Unicode scalar values.
    pub fn new(value: impl Into<String>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        if value.chars().count() > MAX_TRANSCRIPT_SCALARS {
            return Err(RealtimeValueError::TranscriptTooLong);
        }
        Ok(Self(value))
    }

    /// Returns the lossless transcript text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the transcript text, consuming the wrapper.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for TranscriptText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TranscriptText(<redacted>)")
    }
}

impl TryFrom<&str> for TranscriptText {
    type Error = RealtimeValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for TranscriptText {
    type Error = RealtimeValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for TranscriptText {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TranscriptText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(INVALID_TRANSCRIPT_TEXT))?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Opaque function-argument text bounded by UTF-8 bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct FunctionArguments(String);

impl FunctionArguments {
    /// Constructs a delta fragment without requiring complete JSON.
    pub(crate) fn from_delta(value: impl Into<String>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        if value.len() > MAX_VALUE_BYTES {
            return Err(RealtimeValueError::ArgumentsTooLong);
        }
        Ok(Self(value))
    }

    /// Constructs completed arguments that are exactly one JSON object.
    #[allow(dead_code)]
    pub(crate) fn from_completed(value: impl Into<String>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        if value.len() > MAX_VALUE_BYTES {
            return Err(RealtimeValueError::ArgumentsTooLong);
        }
        match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(serde_json::Value::Object(_)) => Ok(Self(value)),
            _ => Err(RealtimeValueError::InvalidArgumentsJson),
        }
    }

    /// Returns the original argument text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the argument text, consuming the wrapper.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for FunctionArguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FunctionArguments(<redacted>)")
    }
}

impl Serialize for FunctionArguments {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FunctionArguments {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(INVALID_FUNCTION_ARGUMENTS))?;
        Self::from_delta(value).map_err(D::Error::custom)
    }
}

/// Opaque provider tool output bounded by UTF-8 bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolOutput(String);

impl ToolOutput {
    /// Constructs tool output with at most 16384 UTF-8 bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        if value.len() > MAX_VALUE_BYTES {
            return Err(RealtimeValueError::ToolOutputTooLong);
        }
        Ok(Self(value))
    }

    /// Returns the original tool output.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the tool output, consuming the wrapper.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for ToolOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ToolOutput(<redacted>)")
    }
}

impl TryFrom<&str> for ToolOutput {
    type Error = RealtimeValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for ToolOutput {
    type Error = RealtimeValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for ToolOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value =
            String::deserialize(deserializer).map_err(|_| D::Error::custom(INVALID_TOOL_OUTPUT))?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// The supported Realtime audio codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    /// RFC 3551 G.711 mu-law audio.
    G711Ulaw,
}

impl AudioCodec {
    /// Returns the exact wire name for this codec.
    pub const fn as_str(self) -> &'static str {
        "g711_ulaw"
    }
}

impl TryFrom<&str> for AudioCodec {
    type Error = RealtimeValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "g711_ulaw" => Ok(Self::G711Ulaw),
            _ => Err(RealtimeValueError::UnsupportedAudioFormat),
        }
    }
}

impl TryFrom<String> for AudioCodec {
    type Error = RealtimeValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl Serialize for AudioCodec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AudioCodec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::UnsupportedAudioFormat))?;
        Self::try_from(value).map_err(D::Error::custom)
    }
}

/// Opaque G.711 mu-law bytes represented by canonical padded base64 on the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct G711Ulaw(Vec<u8>);

impl G711Ulaw {
    /// Constructs nonempty mu-law bytes up to the 16384-byte bound.
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        if value.is_empty() {
            return Err(RealtimeValueError::EmptyAudio);
        }
        if value.len() > MAX_VALUE_BYTES {
            return Err(RealtimeValueError::AudioTooLarge);
        }
        Ok(Self(value))
    }

    /// Returns the opaque mu-law bytes without decoding or reinterpretation.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the mu-law bytes, consuming the wrapper.
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }

    fn from_base64(value: &str) -> Result<Self, RealtimeValueError> {
        if value.len() > MAX_AUDIO_BASE64_CHARS {
            return Err(RealtimeValueError::AudioTooLarge);
        }
        if value.is_empty() || !value.is_ascii() {
            return Err(RealtimeValueError::InvalidBase64);
        }
        let decoded = BASE64_STANDARD
            .decode(value)
            .map_err(|_| RealtimeValueError::InvalidBase64)?;
        if decoded.len() > MAX_VALUE_BYTES {
            return Err(RealtimeValueError::AudioTooLarge);
        }
        if decoded.is_empty() || BASE64_STANDARD.encode(&decoded) != value {
            return Err(RealtimeValueError::InvalidBase64);
        }
        Ok(Self(decoded))
    }
}

impl fmt::Debug for G711Ulaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("G711Ulaw(<redacted>)")
    }
}

impl TryFrom<Vec<u8>> for G711Ulaw {
    type Error = RealtimeValueError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for G711Ulaw {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64_STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for G711Ulaw {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value =
            String::deserialize(deserializer).map_err(|_| D::Error::custom(INVALID_BASE64))?;
        Self::from_base64(&value).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_ids_and_redacted_errors() {
        for value in ["alpha-1", "under_score", "period.dot", "colon:value"] {
            let id = OpaqueId::new(value).expect("accepted opaque id");
            assert_eq!(id.as_str(), value);
            let encoded = serde_json::to_string(&id).expect("serialize opaque id");
            assert_eq!(encoded, format!("\"{value}\""));
            assert_eq!(
                serde_json::from_str::<OpaqueId>(&encoded).expect("deserialize opaque id"),
                id
            );
        }

        for value in ["", "café", "contains space"] {
            assert_eq!(
                OpaqueId::new(value),
                Err(RealtimeValueError::InvalidOpaqueId)
            );
        }
        let exact_limit = "a".repeat(128);
        let id = OpaqueId::new(exact_limit.clone()).expect("128 ASCII bytes are accepted");
        assert_eq!(id.as_str(), exact_limit);
        assert_eq!(id.as_str().len(), 128);
        assert_eq!(
            OpaqueId::new("a".repeat(MAX_ID_BYTES + 1)),
            Err(RealtimeValueError::InvalidOpaqueId)
        );
        assert_eq!(
            serde_json::from_str::<OpaqueId>("\"contains space\"")
                .expect_err("invalid id must be rejected")
                .to_string(),
            INVALID_OPAQUE_ID
        );
        assert!(serde_json::from_str::<OpaqueId>("42").is_err());
        let malformed = serde_json::from_str::<OpaqueId>(r#""unterminated"#)
            .expect_err("malformed JSON syntax must be rejected");
        assert!(!malformed.to_string().contains("unterminated"));

        let errors = [
            (RealtimeValueError::EventTooLarge, "event is too large"),
            (RealtimeValueError::InvalidJson, "invalid JSON"),
            (RealtimeValueError::UnknownEventType, "unknown event type"),
            (
                RealtimeValueError::MissingField("secret_field"),
                "missing required field",
            ),
            (
                RealtimeValueError::InvalidOpaqueId,
                "invalid opaque identifier",
            ),
            (RealtimeValueError::EmptyAudio, "audio is empty"),
            (RealtimeValueError::InvalidBase64, "invalid base64 audio"),
            (RealtimeValueError::AudioTooLarge, "audio is too large"),
            (
                RealtimeValueError::TranscriptTooLong,
                "transcript is too long",
            ),
            (
                RealtimeValueError::ArgumentsTooLong,
                "function arguments are too long",
            ),
            (
                RealtimeValueError::InvalidArgumentsJson,
                "function arguments are not a JSON object",
            ),
            (
                RealtimeValueError::ToolOutputTooLong,
                "tool output is too long",
            ),
            (
                RealtimeValueError::UnsupportedAudioFormat,
                "unsupported audio format",
            ),
            (
                RealtimeValueError::InvalidResponseStatus,
                "invalid response status",
            ),
            (
                RealtimeValueError::InvalidInterruptionReason,
                "invalid interruption reason",
            ),
        ];
        for (error, expected) in errors {
            let display = error.to_string();
            assert_eq!(display, expected);
            assert!(display.chars().count() <= MAX_ERROR_MESSAGE_CHARS);
            for payload in ["secret_field", "contains space", "café"] {
                assert!(!display.contains(payload));
            }
        }
    }

    #[test]
    fn bounded_text_and_g711_ulaw() {
        let transcript_source = "Apt 4B, call 2 — café";
        let transcript = TranscriptText::new(transcript_source).expect("valid transcript");
        assert_eq!(transcript.as_str(), transcript_source);
        assert_eq!(
            serde_json::from_str::<TranscriptText>(
                &serde_json::to_string(&transcript).expect("serialize transcript")
            )
            .expect("deserialize transcript")
            .as_str(),
            transcript_source
        );
        assert!(TranscriptText::new("x".repeat(4096)).is_ok());
        assert_eq!(
            TranscriptText::new("x".repeat(4097)),
            Err(RealtimeValueError::TranscriptTooLong)
        );
        assert_eq!(
            TranscriptText::new("é".repeat(4096))
                .expect("Unicode scalar bound counts scalar values")
                .as_str()
                .chars()
                .count(),
            4096
        );

        let delta_source = r#"{"name":"Ada","arg":"#;
        let delta = FunctionArguments::from_delta(delta_source).expect("delta fragment");
        assert_eq!(delta.as_str(), delta_source);
        assert_eq!(
            serde_json::from_str::<FunctionArguments>(
                &serde_json::to_string(&delta).expect("serialize delta")
            )
            .expect("deserialize delta")
            .as_str(),
            delta_source
        );
        let completed_source = " { \"name\": \"Ada\" } ";
        let completed =
            FunctionArguments::from_completed(completed_source).expect("completed object");
        assert_eq!(completed.as_str(), completed_source);
        for invalid in ["[]", "42", "null", "{malformed", "true"] {
            assert_eq!(
                FunctionArguments::from_completed(invalid),
                Err(RealtimeValueError::InvalidArgumentsJson)
            );
        }
        assert_eq!(
            FunctionArguments::from_completed("x".repeat(16_385)),
            Err(RealtimeValueError::ArgumentsTooLong)
        );
        assert_eq!(
            serde_json::from_str::<FunctionArguments>(&format!("\"{}\"", "x".repeat(16_385)))
                .expect_err("oversized argument serde input")
                .to_string(),
            RealtimeValueError::ArgumentsTooLong.to_string()
        );

        let tool_source = "provider text: \u{0} \n café";
        let tool_output = ToolOutput::new(tool_source).expect("valid tool output");
        assert_eq!(tool_output.as_str(), tool_source);
        assert_eq!(
            serde_json::from_str::<ToolOutput>(
                &serde_json::to_string(&tool_output).expect("serialize tool output")
            )
            .expect("deserialize tool output")
            .as_str(),
            tool_source
        );
        assert_eq!(
            ToolOutput::new("x".repeat(16_385)),
            Err(RealtimeValueError::ToolOutputTooLong)
        );

        assert_eq!(
            serde_json::to_string(&AudioCodec::G711Ulaw).expect("serialize codec"),
            "\"g711_ulaw\""
        );
        assert_eq!(
            serde_json::from_str::<AudioCodec>("\"g711_ulaw\"").expect("deserialize codec"),
            AudioCodec::G711Ulaw
        );
        for unsupported in ["pcm16", "g711_alaw", "", "G711_ULAW", "unknown"] {
            let error = serde_json::from_str::<AudioCodec>(&format!("\"{unsupported}\""))
                .expect_err("unsupported codec must fail")
                .to_string();
            assert_eq!(
                error,
                RealtimeValueError::UnsupportedAudioFormat.to_string()
            );
            if !unsupported.is_empty() {
                assert!(!error.contains(unsupported));
            }
        }

        let audio_bytes = vec![0_u8, 1, 2, 250, 251, 252, 253, 254, 255];
        let audio = G711Ulaw::new(audio_bytes.clone()).expect("valid mu-law bytes");
        assert_eq!(audio.as_bytes(), audio_bytes.as_slice());
        let encoded = serde_json::to_string(&audio).expect("serialize mu-law");
        assert_eq!(encoded, "\"AAEC+vv8/f7/\"");
        let decoded = serde_json::from_str::<G711Ulaw>(&encoded).expect("deserialize mu-law");
        assert_eq!(decoded.as_bytes(), audio_bytes.as_slice());
        assert_eq!(
            G711Ulaw::new(Vec::<u8>::new()),
            Err(RealtimeValueError::EmptyAudio)
        );
        assert_eq!(
            G711Ulaw::new(vec![0_u8; 16_385]),
            Err(RealtimeValueError::AudioTooLarge)
        );

        for invalid in ["", "not-base64", "Zg", "Zg=", "Zg===", "Z g==", "-w=="] {
            let error = serde_json::from_str::<G711Ulaw>(&format!("\"{invalid}\""))
                .expect_err("noncanonical mu-law must fail")
                .to_string();
            assert_eq!(error, RealtimeValueError::InvalidBase64.to_string());
            if !invalid.is_empty() {
                assert!(!error.contains(invalid));
            }
        }
        let oversized = "A".repeat(21_852);
        assert_eq!(
            serde_json::from_str::<G711Ulaw>(&format!("\"{oversized}\""))
                .expect_err("oversized encoded audio must fail")
                .to_string(),
            RealtimeValueError::AudioTooLarge.to_string()
        );

        assert_eq!(
            serde_json::from_str::<TranscriptText>(&format!("\"{}\"", "x".repeat(4097)))
                .expect_err("oversized transcript serde input")
                .to_string(),
            RealtimeValueError::TranscriptTooLong.to_string()
        );
        assert_eq!(
            serde_json::from_str::<ToolOutput>(&format!("\"{}\"", "x".repeat(16_385)))
                .expect_err("oversized tool serde input")
                .to_string(),
            RealtimeValueError::ToolOutputTooLong.to_string()
        );
    }
}
