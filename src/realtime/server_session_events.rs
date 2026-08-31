use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::values::{OpaqueId, RealtimeValueError, TranscriptText};

/// The provider session identity and selected model.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfo {
    pub id: OpaqueId,
    pub model: String,
}

impl fmt::Debug for SessionInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionInfo(<redacted>)")
    }
}

/// A provider error payload whose values remain data at this boundary.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderError {
    #[serde(rename = "type")]
    pub r#type: String,
    pub code: Option<String>,
    pub message: String,
    pub param: Option<String>,
    pub event_id: Option<OpaqueId>,
}

impl fmt::Debug for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderError(<redacted>)")
    }
}

/// Closed server-side session and caller-transcription event values.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum RealtimeServerSessionEvent {
    #[serde(rename = "session.created")]
    SessionCreated {
        event_id: OpaqueId,
        session: SessionInfo,
    },
    #[serde(rename = "session.updated")]
    SessionUpdated {
        event_id: OpaqueId,
        session: SessionInfo,
    },
    #[serde(rename = "error")]
    Error {
        event_id: OpaqueId,
        error: ProviderError,
    },
    #[serde(rename = "input_audio_buffer.committed")]
    InputAudioBufferCommitted {
        event_id: OpaqueId,
        item_id: OpaqueId,
    },
    #[serde(rename = "input_audio_buffer.cleared")]
    InputAudioBufferCleared { event_id: OpaqueId },
    #[serde(rename = "input_audio_buffer.speech_started")]
    InputAudioBufferSpeechStarted {
        event_id: OpaqueId,
        audio_start_ms: u64,
    },
    #[serde(rename = "input_audio_buffer.speech_stopped")]
    InputAudioBufferSpeechStopped {
        event_id: OpaqueId,
        audio_end_ms: u64,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.delta")]
    ConversationItemInputAudioTranscriptionDelta {
        event_id: OpaqueId,
        item_id: OpaqueId,
        content_index: u32,
        delta: TranscriptText,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    ConversationItemInputAudioTranscriptionCompleted {
        event_id: OpaqueId,
        item_id: OpaqueId,
        content_index: u32,
        transcript: TranscriptText,
    },
}

impl fmt::Debug for RealtimeServerSessionEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RealtimeServerSessionEvent")
            .field("type", &self.event_type())
            .finish()
    }
}

impl RealtimeServerSessionEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::SessionCreated { .. } => "session.created",
            Self::SessionUpdated { .. } => "session.updated",
            Self::Error { .. } => "error",
            Self::InputAudioBufferCommitted { .. } => "input_audio_buffer.committed",
            Self::InputAudioBufferCleared { .. } => "input_audio_buffer.cleared",
            Self::InputAudioBufferSpeechStarted { .. } => "input_audio_buffer.speech_started",
            Self::InputAudioBufferSpeechStopped { .. } => "input_audio_buffer.speech_stopped",
            Self::ConversationItemInputAudioTranscriptionDelta { .. } => {
                "conversation.item.input_audio_transcription.delta"
            }
            Self::ConversationItemInputAudioTranscriptionCompleted { .. } => {
                "conversation.item.input_audio_transcription.completed"
            }
        }
    }
}

impl<'de> Deserialize<'de> for RealtimeServerSessionEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_event(&value).map_err(D::Error::custom)
    }
}

fn parse_event(value: &Value) -> Result<RealtimeServerSessionEvent, RealtimeValueError> {
    let object = value.as_object().ok_or(RealtimeValueError::InvalidJson)?;
    let event_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or(RealtimeValueError::UnknownEventType)?;

    match event_type {
        "session.created" => {
            ensure_fields(object, &["type", "event_id", "session"])?;
            Ok(RealtimeServerSessionEvent::SessionCreated {
                event_id: opaque_field(object, "event_id")?,
                session: parse_session(object, "session")?,
            })
        }
        "session.updated" => {
            ensure_fields(object, &["type", "event_id", "session"])?;
            Ok(RealtimeServerSessionEvent::SessionUpdated {
                event_id: opaque_field(object, "event_id")?,
                session: parse_session(object, "session")?,
            })
        }
        "error" => {
            ensure_fields(object, &["type", "event_id", "error"])?;
            Ok(RealtimeServerSessionEvent::Error {
                event_id: opaque_field(object, "event_id")?,
                error: parse_provider_error(object, "error")?,
            })
        }
        "input_audio_buffer.committed" => {
            ensure_fields(object, &["type", "event_id", "item_id"])?;
            Ok(RealtimeServerSessionEvent::InputAudioBufferCommitted {
                event_id: opaque_field(object, "event_id")?,
                item_id: opaque_field(object, "item_id")?,
            })
        }
        "input_audio_buffer.cleared" => {
            ensure_fields(object, &["type", "event_id"])?;
            Ok(RealtimeServerSessionEvent::InputAudioBufferCleared {
                event_id: opaque_field(object, "event_id")?,
            })
        }
        "input_audio_buffer.speech_started" => {
            ensure_fields(object, &["type", "event_id", "audio_start_ms"])?;
            Ok(RealtimeServerSessionEvent::InputAudioBufferSpeechStarted {
                event_id: opaque_field(object, "event_id")?,
                audio_start_ms: u64_field(object, "audio_start_ms")?,
            })
        }
        "input_audio_buffer.speech_stopped" => {
            ensure_fields(object, &["type", "event_id", "audio_end_ms"])?;
            Ok(RealtimeServerSessionEvent::InputAudioBufferSpeechStopped {
                event_id: opaque_field(object, "event_id")?,
                audio_end_ms: u64_field(object, "audio_end_ms")?,
            })
        }
        "conversation.item.input_audio_transcription.delta" => {
            ensure_fields(
                object,
                &["type", "event_id", "item_id", "content_index", "delta"],
            )?;
            Ok(
                RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionDelta {
                    event_id: opaque_field(object, "event_id")?,
                    item_id: opaque_field(object, "item_id")?,
                    content_index: u32_field(object, "content_index")?,
                    delta: transcript_field(object, "delta")?,
                },
            )
        }
        "conversation.item.input_audio_transcription.completed" => {
            ensure_fields(
                object,
                &["type", "event_id", "item_id", "content_index", "transcript"],
            )?;
            Ok(
                RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionCompleted {
                    event_id: opaque_field(object, "event_id")?,
                    item_id: opaque_field(object, "item_id")?,
                    content_index: u32_field(object, "content_index")?,
                    transcript: transcript_field(object, "transcript")?,
                },
            )
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

fn ensure_fields(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), RealtimeValueError> {
    if object
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        return Err(RealtimeValueError::InvalidJson);
    }
    Ok(())
}

fn required<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Value, RealtimeValueError> {
    match object.get(field) {
        Some(value) if !value.is_null() => Ok(value),
        _ => Err(RealtimeValueError::MissingField(field)),
    }
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, RealtimeValueError> {
    required(object, field)?
        .as_object()
        .ok_or(RealtimeValueError::InvalidJson)
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, RealtimeValueError> {
    required(object, field)?
        .as_str()
        .map(str::to_owned)
        .ok_or(RealtimeValueError::InvalidJson)
}

fn opaque_field(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<OpaqueId, RealtimeValueError> {
    let value = required(object, field)?
        .as_str()
        .ok_or(RealtimeValueError::InvalidJson)?;
    OpaqueId::new(value)
}

fn optional_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, RealtimeValueError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RealtimeValueError::InvalidJson),
    }
}

fn optional_opaque(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<OpaqueId>, RealtimeValueError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => OpaqueId::new(value).map(Some),
        Some(_) => Err(RealtimeValueError::InvalidJson),
    }
}

fn parse_session(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<SessionInfo, RealtimeValueError> {
    let session = required_object(object, field)?;
    ensure_fields(session, &["id", "model"])?;
    Ok(SessionInfo {
        id: opaque_field(session, "id")?,
        model: required_string(session, "model")?,
    })
}

fn parse_provider_error(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<ProviderError, RealtimeValueError> {
    let error = required_object(object, field)?;
    ensure_fields(error, &["type", "code", "message", "param", "event_id"])?;
    Ok(ProviderError {
        r#type: required_string(error, "type")?,
        code: optional_string(error, "code")?,
        message: required_string(error, "message")?,
        param: optional_string(error, "param")?,
        event_id: optional_opaque(error, "event_id")?,
    })
}

fn u64_field(object: &Map<String, Value>, field: &'static str) -> Result<u64, RealtimeValueError> {
    required(object, field)?
        .as_u64()
        .ok_or(RealtimeValueError::InvalidJson)
}

fn u32_field(object: &Map<String, Value>, field: &'static str) -> Result<u32, RealtimeValueError> {
    u64_field(object, field)
        .and_then(|value| u32::try_from(value).map_err(|_| RealtimeValueError::InvalidJson))
}

fn transcript_field(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<TranscriptText, RealtimeValueError> {
    let value = required(object, field)?
        .as_str()
        .ok_or(RealtimeValueError::InvalidJson)?;
    TranscriptText::new(value)
}

#[cfg(test)]
mod tests {
    use super::super::values::{OpaqueId, TranscriptText};
    use super::{ProviderError, RealtimeServerSessionEvent, SessionInfo};
    use serde_json::{Value, json};

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).expect("valid opaque id")
    }

    fn error_for(raw: &str, rejected: &[&str]) -> String {
        let error = serde_json::from_str::<RealtimeServerSessionEvent>(raw)
            .expect_err("fixture must be rejected")
            .to_string();
        for secret in rejected {
            assert!(!error.contains(secret), "error leaked fixture: {error}");
        }
        error
    }

    #[test]
    fn session_and_caller_events() {
        let session_created = RealtimeServerSessionEvent::SessionCreated {
            event_id: id("event-created"),
            session: SessionInfo {
                id: id("session-1"),
                model: "gpt-realtime".to_owned(),
            },
        };
        let created_json = serde_json::to_value(&session_created).expect("serialize created");
        assert_eq!(
            created_json,
            json!({
                "type": "session.created",
                "event_id": "event-created",
                "session": {"id": "session-1", "model": "gpt-realtime"},
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerSessionEvent>(created_json)
                .expect("round-trip created"),
            session_created
        );

        let session_updated = RealtimeServerSessionEvent::SessionUpdated {
            event_id: id("event-updated"),
            session: SessionInfo {
                id: id("session-2"),
                model: "gpt-realtime-2026".to_owned(),
            },
        };
        assert_eq!(
            serde_json::from_value::<RealtimeServerSessionEvent>(
                serde_json::to_value(&session_updated).expect("serialize updated"),
            )
            .expect("round-trip updated"),
            session_updated
        );

        let error_event = RealtimeServerSessionEvent::Error {
            event_id: id("event-error"),
            error: ProviderError {
                r#type: "invalid_request_error".to_owned(),
                code: Some("bad_request".to_owned()),
                message: "provider message must remain data".to_owned(),
                param: Some("model".to_owned()),
                event_id: Some(id("provider-event")),
            },
        };
        let error_json = serde_json::to_value(&error_event).expect("serialize error");
        assert_eq!(
            error_json,
            json!({
                "type": "error",
                "event_id": "event-error",
                "error": {
                    "type": "invalid_request_error",
                    "code": "bad_request",
                    "message": "provider message must remain data",
                    "param": "model",
                    "event_id": "provider-event",
                },
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerSessionEvent>(error_json)
                .expect("round-trip error"),
            error_event
        );
        let optional_error = r#"{"type":"error","event_id":"event-error","error":{"type":"server_error","message":"safe"}}"#;
        assert!(serde_json::from_str::<RealtimeServerSessionEvent>(optional_error).is_ok());

        let committed = RealtimeServerSessionEvent::InputAudioBufferCommitted {
            event_id: id("event-committed"),
            item_id: id("item-1"),
        };
        let cleared = RealtimeServerSessionEvent::InputAudioBufferCleared {
            event_id: id("event-cleared"),
        };
        for event in [committed, cleared] {
            let encoded = serde_json::to_value(&event).expect("serialize buffer event");
            assert_eq!(
                serde_json::from_value::<RealtimeServerSessionEvent>(encoded)
                    .expect("round-trip buffer event"),
                event
            );
        }

        let speech_started = RealtimeServerSessionEvent::InputAudioBufferSpeechStarted {
            event_id: id("event-speech-start"),
            audio_start_ms: 42,
        };
        let speech_stopped = RealtimeServerSessionEvent::InputAudioBufferSpeechStopped {
            event_id: id("event-speech-stop"),
            audio_end_ms: 84,
        };
        for event in [speech_started, speech_stopped] {
            let encoded = serde_json::to_value(&event).expect("serialize speech event");
            assert_eq!(
                serde_json::from_value::<RealtimeServerSessionEvent>(encoded)
                    .expect("round-trip speech event"),
                event
            );
        }

        let source = "Apt 4B, call 2 — exact spaces, case, and digits";
        let delta = RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionDelta {
            event_id: id("event-delta"),
            item_id: id("item-transcript"),
            content_index: 3,
            delta: TranscriptText::new(source).expect("valid delta"),
        };
        let completed =
            RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionCompleted {
                event_id: id("event-completed"),
                item_id: id("item-transcript"),
                content_index: 3,
                transcript: TranscriptText::new(source).expect("valid transcript"),
            };
        for event in [delta, completed] {
            let encoded = serde_json::to_value(&event).expect("serialize transcript event");
            assert_eq!(
                serde_json::from_value::<RealtimeServerSessionEvent>(encoded)
                    .expect("round-trip transcript event"),
                event
            );
        }

        for unknown_tag in [
            "response.audio.delta",
            "conversation.item.input_audio_transcription.delta.v2",
        ] {
            let raw = format!(r#"{{"type":"{unknown_tag}"}}"#);
            assert_eq!(error_for(&raw, &[unknown_tag]), "unknown event type");
        }
        for unknown_field in [
            r#"{"type":"session.created","event_id":"event-1","session":{"id":"session-1","model":"safe"},"secret":"payload-secret"}"#,
            r#"{"type":"error","event_id":"event-1","error":{"type":"server_error","message":"safe","secret":"payload-secret"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"event-1","item_id":"item-1","content_index":0,"transcript":"safe","secret":"payload-secret"}"#,
        ] {
            assert_eq!(
                error_for(unknown_field, &["payload-secret"]),
                "invalid JSON"
            );
        }
        for missing in [
            r#"{"type":"session.created","session":{"id":"session-1","model":"safe"}}"#,
            r#"{"type":"input_audio_buffer.committed","event_id":null,"item_id":"item-1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.delta","event_id":"event-1","item_id":"item-1","content_index":0}"#,
            r#"{"type":"error","event_id":"event-1","error":{"type":"server_error"}}"#,
        ] {
            assert_eq!(error_for(missing, &["safe"]), "missing required field");
        }
        let invalid_id = r#"{"type":"session.created","event_id":"bad id","session":{"id":"session-1","model":"safe"}}"#;
        assert_eq!(
            error_for(invalid_id, &["bad id"]),
            "invalid opaque identifier"
        );
        let invalid_nested_id =
            r#"{"type":"input_audio_buffer.committed","event_id":"event-1","item_id":"bad id"}"#;
        assert_eq!(
            error_for(invalid_nested_id, &["bad id"]),
            "invalid opaque identifier"
        );
        let oversized_transcript = "sensitive transcript ".repeat(300);
        let oversized_json = serde_json::to_string(&oversized_transcript).expect("JSON string");
        let oversized_raw = format!(
            r#"{{"type":"conversation.item.input_audio_transcription.completed","event_id":"event-1","item_id":"item-1","content_index":0,"transcript":{oversized_json}}}"#
        );
        assert_eq!(
            error_for(&oversized_raw, &["sensitive transcript"]),
            "transcript is too long"
        );
        let malformed = error_for(
            r#"{"type":"session.created","event_id":"event-1","session":{"id":"session-1","model":"unterminated""#,
            &["unterminated"],
        );
        assert_eq!(malformed, "invalid JSON");

        let debug = format!("{:?}", error_event);
        for secret in [
            "event-error",
            "provider-event",
            "provider message must remain data",
            "invalid_request_error",
        ] {
            assert!(!debug.contains(secret), "debug leaked secret: {debug}");
        }
        let _: Value = serde_json::to_value(&session_created).expect("one JSON object");
    }
}
