use std::fmt;

use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use super::server_audio_events::RealtimeServerAudioEvent;
use super::server_function_events::{
    FunctionCallOutputAckItem, FunctionCallOutputType, RealtimeServerFunctionEvent,
};
use super::server_response_events::{
    InterruptionReason, ProviderErrorSummary, RealtimeServerResponseEvent, ResponseStatus,
    ResponseStatusDetails, ResponseSummary,
};
use super::server_session_events::{ProviderError, RealtimeServerSessionEvent, SessionInfo};
use super::values::{
    FunctionArguments, G711Ulaw, MAX_EVENT_BYTES, OpaqueId, RealtimeValueError, ToolOutput,
    TranscriptText,
};

/// Closed server-side Realtime events grouped by their child contract.
#[derive(Clone, PartialEq, Eq)]
pub enum RealtimeServerEvent {
    /// Session, input-buffer, and caller-transcription events.
    Session(RealtimeServerSessionEvent),
    /// Generated audio and transcript events.
    Audio(RealtimeServerAudioEvent),
    /// Function-call argument and output acknowledgement events.
    Function(RealtimeServerFunctionEvent),
    /// Response completion events.
    Response(RealtimeServerResponseEvent),
}

impl fmt::Debug for RealtimeServerEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealtimeServerEvent(<redacted>)")
    }
}

impl fmt::Display for RealtimeServerEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealtimeServerEvent(<redacted>)")
    }
}

impl Serialize for RealtimeServerEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Session(event) => event.serialize(serializer),
            Self::Audio(event) => event.serialize(serializer),
            Self::Function(event) => event.serialize(serializer),
            Self::Response(event) => event.serialize(serializer),
        }
    }
}

/// Decodes one bounded JSON server event without performing any side effect.
pub fn decode_server_event(bytes: &[u8]) -> Result<RealtimeServerEvent, RealtimeValueError> {
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(RealtimeValueError::EventTooLarge);
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValueSeed
        .deserialize(&mut deserializer)
        .map_err(|_| RealtimeValueError::InvalidJson)?;
    deserializer
        .end()
        .map_err(|_| RealtimeValueError::InvalidJson)?;
    let object = object(value)?;
    parse_event(object)
}

struct UniqueValueSeed;

impl<'de> DeserializeSeed<'de> for UniqueValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom(RealtimeValueError::InvalidJson))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(UniqueValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut map = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if map.contains_key(&key) {
                return Err(A::Error::custom(RealtimeValueError::InvalidJson));
            }
            let value = access.next_value_seed(UniqueValueSeed)?;
            map.insert(key, value);
        }
        Ok(Value::Object(map))
    }
}

fn object(value: Value) -> Result<Map<String, Value>, RealtimeValueError> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn required(
    map: &mut Map<String, Value>,
    field: &'static str,
) -> Result<Value, RealtimeValueError> {
    match map.remove(field) {
        Some(Value::Null) | None => Err(RealtimeValueError::MissingField(field)),
        Some(value) => Ok(value),
    }
}

fn finish(map: Map<String, Value>) -> Result<(), RealtimeValueError> {
    if map.is_empty() {
        Ok(())
    } else {
        Err(RealtimeValueError::InvalidJson)
    }
}

fn ensure_fields(map: &Map<String, Value>, allowed: &[&str]) -> Result<(), RealtimeValueError> {
    if map.keys().any(|field| !allowed.contains(&field.as_str())) {
        return Err(RealtimeValueError::InvalidJson);
    }
    Ok(())
}

fn string(value: Value) -> Result<String, RealtimeValueError> {
    match value {
        Value::String(value) => Ok(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn opaque_id(value: Value) -> Result<OpaqueId, RealtimeValueError> {
    match value {
        Value::String(value) => OpaqueId::new(value),
        _ => Err(RealtimeValueError::InvalidOpaqueId),
    }
}

fn session_opaque_id(value: Value) -> Result<OpaqueId, RealtimeValueError> {
    match value {
        Value::String(value) => OpaqueId::new(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn optional_session_opaque_id(
    map: &mut Map<String, Value>,
    field: &'static str,
) -> Result<Option<OpaqueId>, RealtimeValueError> {
    match map.remove(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => session_opaque_id(value).map(Some),
    }
}

fn optional_string(
    map: &mut Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, RealtimeValueError> {
    match map.remove(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(RealtimeValueError::InvalidJson),
    }
}

fn index(value: Value) -> Result<u32, RealtimeValueError> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(RealtimeValueError::InvalidJson)
}

fn timestamp(value: Value) -> Result<u64, RealtimeValueError> {
    value.as_u64().ok_or(RealtimeValueError::InvalidJson)
}

fn transcript(value: Value) -> Result<TranscriptText, RealtimeValueError> {
    match value {
        Value::String(value) => TranscriptText::new(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn audio(value: Value) -> Result<G711Ulaw, RealtimeValueError> {
    match value {
        Value::String(_) => serde_json::from_value::<G711Ulaw>(value).map_err(|error| match error
            .to_string()
            .as_str()
        {
            "audio is too large" => RealtimeValueError::AudioTooLarge,
            "audio is empty" => RealtimeValueError::EmptyAudio,
            _ => RealtimeValueError::InvalidBase64,
        }),
        _ => Err(RealtimeValueError::InvalidBase64),
    }
}

fn arguments_delta(value: Value) -> Result<FunctionArguments, RealtimeValueError> {
    match value {
        Value::String(value) => FunctionArguments::from_delta(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn arguments_completed(value: Value) -> Result<FunctionArguments, RealtimeValueError> {
    match value {
        Value::String(value) => FunctionArguments::from_completed(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn tool_output(value: Value) -> Result<ToolOutput, RealtimeValueError> {
    match value {
        Value::String(value) => ToolOutput::new(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn parse_session_info(value: Value) -> Result<SessionInfo, RealtimeValueError> {
    let mut map = object(value)?;
    ensure_fields(&map, &["id", "model"])?;
    let id = session_opaque_id(required(&mut map, "id")?)?;
    let model = string(required(&mut map, "model")?)?;
    finish(map)?;
    Ok(SessionInfo { id, model })
}

fn parse_provider_error(value: Value) -> Result<ProviderError, RealtimeValueError> {
    let mut map = object(value)?;
    ensure_fields(&map, &["type", "code", "message", "param", "event_id"])?;
    let r#type = string(required(&mut map, "type")?)?;
    let code = optional_string(&mut map, "code")?;
    let message = string(required(&mut map, "message")?)?;
    let param = optional_string(&mut map, "param")?;
    let event_id = optional_session_opaque_id(&mut map, "event_id")?;
    finish(map)?;
    Ok(ProviderError {
        r#type,
        code,
        message,
        param,
        event_id,
    })
}

fn parse_function_item(value: Value) -> Result<FunctionCallOutputAckItem, RealtimeValueError> {
    let mut map = object(value)?;
    let id = opaque_id(required(&mut map, "id")?)?;
    let r#type = match required(&mut map, "type")? {
        Value::String(value) if value == "function_call_output" => {
            FunctionCallOutputType::FunctionCallOutput
        }
        _ => return Err(RealtimeValueError::UnknownEventType),
    };
    let call_id = opaque_id(required(&mut map, "call_id")?)?;
    let output = tool_output(required(&mut map, "output")?)?;
    finish(map)?;
    Ok(FunctionCallOutputAckItem {
        id,
        r#type,
        call_id,
        output,
    })
}

fn parse_status(value: Value) -> Result<ResponseStatus, RealtimeValueError> {
    match value {
        Value::String(value) => ResponseStatus::try_from(value),
        _ => Err(RealtimeValueError::InvalidResponseStatus),
    }
}

fn parse_reason(value: Value) -> Result<InterruptionReason, RealtimeValueError> {
    match value {
        Value::String(value) => InterruptionReason::try_from(value),
        _ => Err(RealtimeValueError::InvalidInterruptionReason),
    }
}

fn parse_provider_error_summary(value: Value) -> Result<ProviderErrorSummary, RealtimeValueError> {
    let mut map = object(value)?;
    let r#type = string(required(&mut map, "type")?)?;
    let code = optional_string(&mut map, "code")?;
    finish(map)?;
    Ok(ProviderErrorSummary { r#type, code })
}

fn parse_status_details(value: Value) -> Result<ResponseStatusDetails, RealtimeValueError> {
    let mut map = object(value)?;
    let reason = match map.remove("reason") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_reason(value)?),
    };
    let error = match map.remove("error") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_provider_error_summary(value)?),
    };
    finish(map)?;
    Ok(ResponseStatusDetails { reason, error })
}

fn parse_response_summary(value: Value) -> Result<ResponseSummary, RealtimeValueError> {
    let mut map = object(value)?;
    let id = opaque_id(required(&mut map, "id")?)?;
    let status = parse_status(required(&mut map, "status")?)?;
    let status_details = match map.remove("status_details") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_status_details(value)?),
    };
    finish(map)?;
    ResponseSummary::new(id, status, status_details)
}

fn parse_event(mut map: Map<String, Value>) -> Result<RealtimeServerEvent, RealtimeValueError> {
    let kind = match map.remove("type") {
        Some(Value::String(value)) => value,
        _ => return Err(RealtimeValueError::UnknownEventType),
    };

    match kind.as_str() {
        "session.created" => {
            ensure_fields(&map, &["event_id", "session"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let session = parse_session_info(required(&mut map, "session")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::SessionCreated { event_id, session },
            ))
        }
        "session.updated" => {
            ensure_fields(&map, &["event_id", "session"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let session = parse_session_info(required(&mut map, "session")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::SessionUpdated { event_id, session },
            ))
        }
        "error" => {
            ensure_fields(&map, &["event_id", "error"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let error = parse_provider_error(required(&mut map, "error")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::Error { event_id, error },
            ))
        }
        "input_audio_buffer.committed" => {
            ensure_fields(&map, &["event_id", "item_id"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let item_id = session_opaque_id(required(&mut map, "item_id")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::InputAudioBufferCommitted { event_id, item_id },
            ))
        }
        "input_audio_buffer.cleared" => {
            ensure_fields(&map, &["event_id"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::InputAudioBufferCleared { event_id },
            ))
        }
        "input_audio_buffer.speech_started" => {
            ensure_fields(&map, &["event_id", "audio_start_ms"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let audio_start_ms = timestamp(required(&mut map, "audio_start_ms")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::InputAudioBufferSpeechStarted {
                    event_id,
                    audio_start_ms,
                },
            ))
        }
        "input_audio_buffer.speech_stopped" => {
            ensure_fields(&map, &["event_id", "audio_end_ms"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let audio_end_ms = timestamp(required(&mut map, "audio_end_ms")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::InputAudioBufferSpeechStopped {
                    event_id,
                    audio_end_ms,
                },
            ))
        }
        "conversation.item.input_audio_transcription.delta" => {
            ensure_fields(&map, &["event_id", "item_id", "content_index", "delta"])?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let item_id = session_opaque_id(required(&mut map, "item_id")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let delta = transcript(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionDelta {
                    event_id,
                    item_id,
                    content_index,
                    delta,
                },
            ))
        }
        "conversation.item.input_audio_transcription.completed" => {
            ensure_fields(
                &map,
                &["event_id", "item_id", "content_index", "transcript"],
            )?;
            let event_id = session_opaque_id(required(&mut map, "event_id")?)?;
            let item_id = session_opaque_id(required(&mut map, "item_id")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let transcript = transcript(required(&mut map, "transcript")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Session(
                RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionCompleted {
                    event_id,
                    item_id,
                    content_index,
                    transcript,
                },
            ))
        }
        "response.output_audio.delta" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let delta = audio(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Audio(
                RealtimeServerAudioEvent::OutputAudioDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    delta,
                },
            ))
        }
        "response.output_audio.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Audio(
                RealtimeServerAudioEvent::OutputAudioDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                },
            ))
        }
        "response.output_audio_transcript.delta" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let delta = transcript(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Audio(
                RealtimeServerAudioEvent::OutputAudioTranscriptDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    delta,
                },
            ))
        }
        "response.output_audio_transcript.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let transcript = transcript(required(&mut map, "transcript")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Audio(
                RealtimeServerAudioEvent::OutputAudioTranscriptDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    transcript,
                },
            ))
        }
        "response.function_call_arguments.delta" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let call_id = opaque_id(required(&mut map, "call_id")?)?;
            let delta = arguments_delta(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Function(
                RealtimeServerFunctionEvent::FunctionCallArgumentsDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    call_id,
                    delta,
                },
            ))
        }
        "response.function_call_arguments.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let call_id = opaque_id(required(&mut map, "call_id")?)?;
            let name = string(required(&mut map, "name")?)?;
            let arguments = arguments_completed(required(&mut map, "arguments")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Function(
                RealtimeServerFunctionEvent::FunctionCallArgumentsDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    call_id,
                    name,
                    arguments,
                },
            ))
        }
        "conversation.item.created" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let item = parse_function_item(required(&mut map, "item")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Function(
                RealtimeServerFunctionEvent::ConversationItemCreated { event_id, item },
            ))
        }
        "response.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response = parse_response_summary(required(&mut map, "response")?)?;
            finish(map)?;
            Ok(RealtimeServerEvent::Response(
                RealtimeServerResponseEvent::ResponseDone { event_id, response },
            ))
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use serde_json::{Value, json};

    use super::super::client_events::RealtimeClientEvent;
    use super::super::server_audio_events::RealtimeServerAudioEvent;
    use super::super::server_function_events::RealtimeServerFunctionEvent;
    use super::super::server_response_events::RealtimeServerResponseEvent;
    use super::super::server_session_events::RealtimeServerSessionEvent;
    use super::super::values::{MAX_EVENT_BYTES, OpaqueId, RealtimeValueError};
    use super::{RealtimeServerEvent, decode_server_event};

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).expect("test identifier is valid")
    }

    fn assert_wire(
        raw: &str,
        expected_type: &str,
        predicate: impl FnOnce(&RealtimeServerEvent) -> bool,
    ) {
        let event = decode_server_event(raw.as_bytes()).expect("valid server event");
        assert!(predicate(&event));
        assert_eq!(
            serde_json::to_value(&event).expect("serialize server event"),
            serde_json::from_str::<Value>(raw).expect("fixture JSON"),
        );
        assert_eq!(
            serde_json::to_value(&event).expect("serialized object")["type"],
            expected_type,
        );
    }

    #[test]
    fn closed_server_dispatch_matrix() {
        assert_wire(
            r#"{"type":"session.created","event_id":"event-created","session":{"id":"session-1","model":"gpt-realtime"}}"#,
            "session.created",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(RealtimeServerSessionEvent::SessionCreated { .. })
                )
            },
        );
        assert_wire(
            r#"{"type":"session.updated","event_id":"event-updated","session":{"id":"session-2","model":"gpt-realtime-2026"}}"#,
            "session.updated",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(RealtimeServerSessionEvent::SessionUpdated { .. })
                )
            },
        );
        assert_wire(
            r#"{"type":"error","event_id":"event-error","error":{"type":"server_error","code":"E-42","message":"provider message","param":"model","event_id":"provider-event"}}"#,
            "error",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(RealtimeServerSessionEvent::Error { .. })
                )
            },
        );
        assert_wire(
            r#"{"type":"input_audio_buffer.committed","event_id":"event-committed","item_id":"item-1"}"#,
            "input_audio_buffer.committed",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::InputAudioBufferCommitted { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"input_audio_buffer.cleared","event_id":"event-cleared"}"#,
            "input_audio_buffer.cleared",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::InputAudioBufferCleared { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"input_audio_buffer.speech_started","event_id":"event-speech-start","audio_start_ms":42}"#,
            "input_audio_buffer.speech_started",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::InputAudioBufferSpeechStarted { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"input_audio_buffer.speech_stopped","event_id":"event-speech-stop","audio_end_ms":84}"#,
            "input_audio_buffer.speech_stopped",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::InputAudioBufferSpeechStopped { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"conversation.item.input_audio_transcription.delta","event_id":"event-transcript-delta","item_id":"item-transcript","content_index":3,"delta":"Apt 4B, call 2 — exact"}"#,
            "conversation.item.input_audio_transcription.delta",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionDelta { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"event-transcript-done","item_id":"item-transcript","content_index":3,"transcript":"Apt 4B, call 2 — exact"}"#,
            "conversation.item.input_audio_transcription.completed",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionCompleted {
                            ..
                        }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"response.output_audio.delta","event_id":"event-audio-delta","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g=="}"#,
            "response.output_audio.delta",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Audio(RealtimeServerAudioEvent::OutputAudioDelta { .. })
                )
            },
        );
        assert_wire(
            r#"{"type":"response.output_audio.done","event_id":"event-audio-done","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            "response.output_audio.done",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Audio(RealtimeServerAudioEvent::OutputAudioDone { .. })
                )
            },
        );
        assert_wire(
            r#"{"type":"response.output_audio_transcript.delta","event_id":"event-audio-transcript-delta","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"Apt 4B, call 2"}"#,
            "response.output_audio_transcript.delta",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Audio(
                        RealtimeServerAudioEvent::OutputAudioTranscriptDelta { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-audio-transcript-done","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"transcript":"Apt 4B, call 2"}"#,
            "response.output_audio_transcript.done",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Audio(
                        RealtimeServerAudioEvent::OutputAudioTranscriptDone { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"response.function_call_arguments.delta","event_id":"event-arguments-delta","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","delta":"{\"city\":\"Syd\""}"#,
            "response.function_call_arguments.delta",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Function(
                        RealtimeServerFunctionEvent::FunctionCallArgumentsDelta { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"response.function_call_arguments.done","event_id":"event-arguments-done","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","name":"get_weather","arguments":" { \"city\": \"Sydney\" } "}"#,
            "response.function_call_arguments.done",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Function(
                        RealtimeServerFunctionEvent::FunctionCallArgumentsDone { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"conversation.item.created","event_id":"event-item-created","item":{"id":"item-1","type":"function_call_output","call_id":"call-1","output":"provider output: café ✓"}}"#,
            "conversation.item.created",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Function(
                        RealtimeServerFunctionEvent::ConversationItemCreated { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"response.done","event_id":"event-response-done","response":{"id":"response-1","status":"completed","status_details":{"reason":null,"error":{"type":"provider_error","code":"E-42"}}}}"#,
            "response.done",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Response(RealtimeServerResponseEvent::ResponseDone { .. })
                )
            },
        );

        let client = RealtimeClientEvent::ResponseCancel {
            event_id: Some(id("client-event-1")),
        };
        let client_value = serde_json::to_value(&client).expect("serialize client smoke");
        assert_eq!(
            client_value,
            json!({"type":"response.cancel","event_id":"client-event-1"})
        );
        assert_eq!(
            serde_json::from_value::<RealtimeClientEvent>(client_value.clone())
                .expect("client round trip"),
            client
        );
        assert_eq!(
            decode_server_event(&serde_json::to_vec(&client_value).expect("client JSON")),
            Err(RealtimeValueError::UnknownEventType)
        );

        for raw in [
            r#"{"type":"response.audio.delta"}"#,
            r#"{"type":"response.audio.done"}"#,
            r#"{"type":"undocumented.extension"}"#,
        ] {
            assert_eq!(
                decode_server_event(raw.as_bytes()),
                Err(RealtimeValueError::UnknownEventType)
            );
        }
        for raw in [
            r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"content_index":3}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"completed","status":"failed","status_details":null}}"#,
            r#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":"function_call_output","call_id":"call-1","output":"safe","output":"secret"}}"#,
            r#"{"type":"session.created","event_id":"event-1","session":{"id":"session-1","model":"safe","model":"secret"}}"#,
        ] {
            assert_eq!(
                decode_server_event(raw.as_bytes()),
                Err(RealtimeValueError::InvalidJson)
            );
        }
        for raw in [
            &b"{"[..],
            &br"[]"[..],
            &br"null"[..],
            &br#""not an event""#[..],
        ] {
            assert_eq!(
                decode_server_event(raw),
                Err(RealtimeValueError::InvalidJson)
            );
        }
        for raw in [
            &br"{}"[..],
            &br#"{"type":null}"#[..],
            &br#"{"type":42}"#[..],
        ] {
            assert_eq!(
                decode_server_event(raw),
                Err(RealtimeValueError::UnknownEventType)
            );
        }
        assert_eq!(
            decode_server_event(
                br#"{"type":"response.output_audio.done","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            ),
            Err(RealtimeValueError::MissingField("event_id"))
        );
        assert_eq!(
            decode_server_event(
                br#"{"type":"response.output_audio.done","event_id":"bad id","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            ),
            Err(RealtimeValueError::InvalidOpaqueId)
        );
        for raw in [
            &br#"{"type":"response.output_audio.done","event_id":"event-1","response_id":42,"item_id":"item-1","output_index":1,"content_index":2}"#[..],
            &br#"{"type":"response.function_call_arguments.delta","event_id":"event-1","response_id":42,"item_id":"item-1","output_index":1,"call_id":"call-1","delta":"{}"}"#[..],
            &br#"{"type":"response.done","event_id":"event-1","response":{"id":42,"status":"completed","status_details":null}}"#[..],
        ] {
            assert_eq!(
                decode_server_event(raw),
                Err(RealtimeValueError::InvalidOpaqueId)
            );
        }
        for raw in [
            &br#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":42}"#[..],
            &br#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":{}}"#[..],
            &br#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":[]}"#[..],
        ] {
            assert_eq!(
                decode_server_event(raw),
                Err(RealtimeValueError::InvalidBase64)
            );
        }
        for raw in [
            &br#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":42,"call_id":"call-1","output":"safe"}}"#[..],
            &br#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":{},"call_id":"call-1","output":"safe"}}"#[..],
            &br#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":[],"call_id":"call-1","output":"safe"}}"#[..],
        ] {
            assert_eq!(
                decode_server_event(raw),
                Err(RealtimeValueError::UnknownEventType)
            );
        }
        for (raw, missing_field) in [
            (
                &br#"{"type":"response.output_audio.done","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"unexpected":"secret"}"#[..],
                "event_id",
            ),
            (
                &br#"{"type":"response.function_call_arguments.delta","response_id":"response-1","item_id":"item-1","output_index":1,"call_id":"call-1","delta":"{}","unexpected":"secret"}"#[..],
                "event_id",
            ),
            (
                &br#"{"type":"conversation.item.created","event_id":"event-1","item":{"type":"function_call_output","call_id":"call-1","output":"safe","unexpected":"secret"}}"#[..],
                "id",
            ),
            (
                &br#"{"type":"response.done","event_id":"event-1","response":{"status":"completed","status_details":null,"unexpected":"secret"}}"#[..],
                "id",
            ),
            (
                &br#"{"type":"response.done","response":{"id":"response-1","status":"completed","status_details":null},"unexpected":"secret"}"#[..],
                "event_id",
            ),
        ] {
            assert_eq!(
                decode_server_event(raw),
                Err(RealtimeValueError::MissingField(missing_field))
            );
        }
        assert_eq!(
            decode_server_event(
                br#"{"type":"session.created","event_id":42,"session":{"id":"session-1","model":"gpt-realtime"}}"#,
            ),
            Err(RealtimeValueError::InvalidJson)
        );

        let oversized_event = vec![b' '; MAX_EVENT_BYTES + 1];
        assert_eq!(
            decode_server_event(&oversized_event),
            Err(RealtimeValueError::EventTooLarge)
        );
        let oversized_transcript = "x".repeat(4_097);
        let raw = json!({
            "type": "response.output_audio_transcript.done",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2,
            "transcript": oversized_transcript,
        });
        assert_eq!(
            decode_server_event(&serde_json::to_vec(&raw).expect("oversized transcript JSON")),
            Err(RealtimeValueError::TranscriptTooLong)
        );
        let oversized_arguments = "x".repeat(16_385);
        let raw = json!({
            "type": "response.function_call_arguments.done",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "call_id": "call-1",
            "name": "safe",
            "arguments": oversized_arguments,
        });
        assert_eq!(
            decode_server_event(&serde_json::to_vec(&raw).expect("oversized arguments JSON")),
            Err(RealtimeValueError::ArgumentsTooLong)
        );
        let oversized_output = "x".repeat(16_385);
        let raw = json!({
            "type": "conversation.item.created",
            "event_id": "event-1",
            "item": {
                "id": "item-1",
                "type": "function_call_output",
                "call_id": "call-1",
                "output": oversized_output,
            },
        });
        assert_eq!(
            decode_server_event(&serde_json::to_vec(&raw).expect("oversized output JSON")),
            Err(RealtimeValueError::ToolOutputTooLong)
        );
        let oversized_audio = BASE64_STANDARD.encode(vec![0x2a; 16_385]);
        let raw = json!({
            "type": "response.output_audio.delta",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2,
            "delta": oversized_audio,
        });
        assert_eq!(
            decode_server_event(&serde_json::to_vec(&raw).expect("oversized audio JSON")),
            Err(RealtimeValueError::AudioTooLarge)
        );
        assert_eq!(
            decode_server_event(
                br#"{"type":"response.function_call_arguments.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"call_id":"call-1","name":"safe","arguments":"[]"}"#,
            ),
            Err(RealtimeValueError::InvalidArgumentsJson)
        );
        assert_eq!(
            decode_server_event(
                br#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"secret-status","status_details":null}}"#,
            ),
            Err(RealtimeValueError::InvalidResponseStatus)
        );
        assert_eq!(
            decode_server_event(
                br#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"completed","status_details":{"reason":"secret-reason","error":null}}}"#,
            ),
            Err(RealtimeValueError::InvalidInterruptionReason)
        );

        let redacted = decode_server_event(
            br#"{"type":"response.output_audio.done","event_id":"event-secret","response_id":"response-secret","item_id":"item-secret","output_index":1,"content_index":2}"#,
        )
        .expect("redaction fixture");
        let debug = format!("{redacted:?}");
        let display = redacted.to_string();
        for secret in ["event-secret", "response-secret", "item-secret"] {
            assert!(!debug.contains(secret));
            assert!(!display.contains(secret));
        }
    }
}
