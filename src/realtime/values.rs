//! Bounded, provider-independent values used by the Realtime boundary.

use std::fmt;

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum bytes accepted for one provider event.
pub const MAX_EVENT_BYTES: usize = 65_536;
/// Maximum bytes accepted for one opaque identifier.
pub const MAX_ID_BYTES: usize = 128;
/// Maximum characters emitted by a value error.
pub const MAX_ERROR_MESSAGE_CHARS: usize = 512;

const INVALID_OPAQUE_ID: &str = "realtime_invalid_opaque_id";

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
}
