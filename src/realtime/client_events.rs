#[cfg(test)]
mod tests {
    use super::super::values::{AudioCodec, G711Ulaw, OpaqueId, ToolOutput};
    use super::{
        FunctionCallOutputItem, FunctionCallOutputType, RealtimeClientEvent, SessionUpdatePayload,
        TurnDetection,
    };
    use serde_json::{Value, json};

    #[test]
    fn closed_client_events() {
        let model = "gpt-realtime-2026-08".to_owned();
        let event_id = OpaqueId::new("event-1").expect("valid event id");
        let item_id = OpaqueId::new("item-1").expect("valid item id");
        let call_id = OpaqueId::new("call-1").expect("valid call id");
        let audio = G711Ulaw::new(vec![0, 1, 2, 250]).expect("valid audio");
        let output = ToolOutput::new("provider output: café ✓").expect("valid output");

        for vad in ["server_vad", "semantic_vad"] {
            let turn_detection = TurnDetection::new(vad).expect("accepted VAD");
            let session = SessionUpdatePayload {
                model: model.clone(),
                input_audio_format: AudioCodec::G711Ulaw,
                output_audio_format: AudioCodec::G711Ulaw,
                turn_detection: Some(turn_detection),
            };
            let event = RealtimeClientEvent::SessionUpdate {
                event_id: Some(event_id.clone()),
                session,
            };
            let encoded = serde_json::to_value(&event).expect("serialize session update");
            assert_eq!(
                encoded,
                json!({
                    "type": "session.update",
                    "event_id": "event-1",
                    "session": {
                        "model": model,
                        "input_audio_format": "g711_ulaw",
                        "output_audio_format": "g711_ulaw",
                        "turn_detection": {"type": vad}
                    }
                })
            );
            assert_eq!(
                serde_json::from_value::<RealtimeClientEvent>(encoded).expect("round trip"),
                event
            );
        }

        let session_without_optional = SessionUpdatePayload {
            model: "exact model with spaces".to_owned(),
            input_audio_format: AudioCodec::G711Ulaw,
            output_audio_format: AudioCodec::G711Ulaw,
            turn_detection: None,
        };
        let session_without_id = RealtimeClientEvent::SessionUpdate {
            event_id: None,
            session: session_without_optional,
        };
        let session_without_id_json = serde_json::to_value(&session_without_id).expect("serialize");
        assert_eq!(
            session_without_id_json,
            json!({
                "type": "session.update",
                "session": {
                    "model": "exact model with spaces",
                    "input_audio_format": "g711_ulaw",
                    "output_audio_format": "g711_ulaw"
                }
            })
        );
        assert_eq!(
            serde_json::from_str::<RealtimeClientEvent>(
                r#"{"type":"session.update","event_id":null,"session":{"model":"exact model with spaces","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":null}}"#
            )
            .expect("null optional fields"),
            session_without_id
        );

        let append = RealtimeClientEvent::InputAudioBufferAppend {
            event_id: Some(event_id.clone()),
            audio: audio.clone(),
        };
        let append_json = serde_json::to_value(&append).expect("serialize append");
        assert_eq!(
            append_json,
            json!({
                "type": "input_audio_buffer.append",
                "event_id": "event-1",
                "audio": "AAEC+g=="
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeClientEvent>(append_json).expect("round trip"),
            append
        );
        assert_eq!(
            serde_json::from_str::<RealtimeClientEvent>(
                r#"{"type":"input_audio_buffer.append","event_id":null,"audio":"AAEC+g=="}"#
            )
            .expect("null append id"),
            RealtimeClientEvent::InputAudioBufferAppend {
                event_id: None,
                audio: audio.clone()
            }
        );

        for (event, expected) in [
            (
                RealtimeClientEvent::InputAudioBufferCommit {
                    event_id: Some(event_id.clone()),
                },
                json!({"type":"input_audio_buffer.commit","event_id":"event-1"}),
            ),
            (
                RealtimeClientEvent::InputAudioBufferClear { event_id: None },
                json!({"type":"input_audio_buffer.clear"}),
            ),
            (
                RealtimeClientEvent::ResponseCancel {
                    event_id: Some(event_id.clone()),
                },
                json!({"type":"response.cancel","event_id":"event-1"}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(&event).expect("serialize event"),
                expected
            );
            let round_trip =
                serde_json::from_value::<RealtimeClientEvent>(expected).expect("round trip event");
            assert_eq!(round_trip, event);
        }

        let item = FunctionCallOutputItem {
            id: Some(item_id),
            r#type: FunctionCallOutputType::FunctionCallOutput,
            call_id,
            output,
        };
        let conversation = RealtimeClientEvent::ConversationItemCreate {
            event_id: Some(event_id),
            item,
        };
        let conversation_json = serde_json::to_value(&conversation).expect("serialize item");
        assert_eq!(
            conversation_json,
            json!({
                "type": "conversation.item.create",
                "event_id": "event-1",
                "item": {
                    "id": "item-1",
                    "type": "function_call_output",
                    "call_id": "call-1",
                    "output": "provider output: café ✓"
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeClientEvent>(conversation_json).expect("round trip"),
            conversation
        );
        let item_without_id = json!({
            "type": "conversation.item.create",
            "item": {
                "id": null,
                "type": "function_call_output",
                "call_id": "call-2",
                "output": "lossless output"
            }
        });
        let decoded_without_id =
            serde_json::from_value::<RealtimeClientEvent>(item_without_id).expect("null item id");
        assert_eq!(
            serde_json::to_value(decoded_without_id).expect("serialize null item id"),
            json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "function_call_output",
                    "call_id": "call-2",
                    "output": "lossless output"
                }
            })
        );

        let redacted = |raw: &str, rejected: &str| -> String {
            let error = serde_json::from_str::<RealtimeClientEvent>(raw)
                .expect_err("fixture must be rejected")
                .to_string();
            if !rejected.is_empty() {
                assert!(
                    !error.contains(rejected),
                    "error leaked rejected fixture: {error}"
                );
            }
            error
        };

        for unsupported in ["pcm16", "g711_alaw", "", "G711_ULAW", "codec-secret"] {
            let error = redacted(
                &format!(
                    "{{\"type\":\"session.update\",\"session\":{{\"model\":\"safe\",\"input_audio_format\":\"{unsupported}\",\"output_audio_format\":\"g711_ulaw\"}}}}"
                ),
                unsupported,
            );
            assert!(error.contains("unsupported audio format"));
            if !unsupported.is_empty() {
                assert!(!error.contains(unsupported));
            }
        }
        let vad_error = redacted(
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":{"type":"client_vad"}}}"#,
            "client_vad",
        );
        assert!(vad_error.contains("invalid JSON"));
        assert_eq!(
            TurnDetection::new("client_vad"),
            Err(super::super::values::RealtimeValueError::InvalidJson)
        );
        for invalid_vad in [
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":{}}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":{"type":null}}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":[]}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":{"type":"server_vad","extra":"secret"}}}"#,
        ] {
            let error = redacted(invalid_vad, "secret");
            assert!(!error.is_empty());
        }
        let item_type_error = redacted(
            r#"{"type":"conversation.item.create","item":{"type":"message","call_id":"call-1","output":"safe"}}"#,
            "message",
        );
        assert!(item_type_error.contains("unknown event type"));
        for unknown_tag in [
            "response.create",
            "conversation.item.delete",
            "secret.event",
        ] {
            let error = redacted(&format!("{{\"type\":\"{unknown_tag}\"}}"), unknown_tag);
            assert!(error.contains("unknown event type"));
        }
        for unknown_field in [
            r#"{"type":"input_audio_buffer.commit","secret":"payload-secret"}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","unknown":"payload-secret"}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw","turn_detection":{"type":"server_vad","unknown":"payload-secret"}}}"#,
            r#"{"type":"conversation.item.create","item":{"type":"function_call_output","call_id":"call-1","output":"safe","unknown":"payload-secret"}}"#,
        ] {
            let error = redacted(unknown_field, "payload-secret");
            assert!(error.contains("invalid JSON"));
        }
        for malformed in [
            r#"{"type":"session.update","session":[]}"#,
            r#"{"type":"input_audio_buffer.append","audio":{}}"#,
            r#"{"type":"conversation.item.create","item":[]}"#,
            r#"{"type":"conversation.item.create","item":{"type":"function_call_output","call_id":"call-1","output":null}}"#,
            r#"{"type":"input_audio_buffer.append","audio":"not-base64"}"#,
        ] {
            let error = redacted(malformed, "not-base64");
            assert!(!error.contains("{"));
        }
        let oversized_output = format!(
            "{{\"type\":\"conversation.item.create\",\"item\":{{\"type\":\"function_call_output\",\"call_id\":\"call-1\",\"output\":\"{}\"}}}}",
            "x".repeat(16_385)
        );
        let oversized_error = redacted(&oversized_output, "xxxxxxxx");
        assert!(oversized_error.contains("tool output is too long"));

        for missing in [
            r#"{"type":"session.update"}"#,
            r#"{"type":"session.update","session":null}"#,
            r#"{"type":"session.update","session":{"input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw"}}"#,
            r#"{"type":"session.update","session":{"model":null,"input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw"}}"#,
            r#"{"type":"session.update","session":{"model":"safe","output_audio_format":"g711_ulaw"}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":null,"output_audio_format":"g711_ulaw"}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw"}}"#,
            r#"{"type":"session.update","session":{"model":"safe","input_audio_format":"g711_ulaw","output_audio_format":null}}"#,
            r#"{"type":"input_audio_buffer.append"}"#,
            r#"{"type":"input_audio_buffer.append","audio":null}"#,
            r#"{"type":"conversation.item.create","item":{"type":"function_call_output","output":"safe"}}"#,
            r#"{"type":"conversation.item.create","item":{"type":"function_call_output","call_id":null,"output":"safe"}}"#,
            r#"{"type":"conversation.item.create","item":{"type":"function_call_output","call_id":"call-1"}}"#,
            r#"{"type":"conversation.item.create","item":{"type":"function_call_output","call_id":"call-1","output":null}}"#,
        ] {
            let error = redacted(missing, "safe");
            assert!(error.contains("missing required field") || error.contains("invalid JSON"));
        }
        for missing_type in [
            r#"{"type":"conversation.item.create","item":{"call_id":"call-1","output":"safe"}}"#,
            r#"{"type":"conversation.item.create","item":{"type":null,"call_id":"call-1","output":"safe"}}"#,
        ] {
            let error = redacted(missing_type, "safe");
            assert!(error.contains("missing required field"));
        }
        for missing_event_type in ["{}", r#"{"type":null}"#, r#"{"type":42}"#] {
            let error = redacted(missing_event_type, "42");
            assert!(error.contains("unknown event type"));
        }
        let invalid_id_error = redacted(
            r#"{"type":"input_audio_buffer.commit","event_id":"bad id"}"#,
            "bad id",
        );
        assert!(invalid_id_error.contains("invalid opaque identifier"));

        let debug = format!("{:?}", conversation);
        for secret in [
            "event-1",
            "item-1",
            "call-1",
            "provider output: café ✓",
            "gpt-realtime-2026-08",
        ] {
            assert!(!debug.contains(secret), "debug leaked secret: {debug}");
        }
        let _: Value = serde_json::from_str(&serde_json::to_string(&append).expect("JSON object"))
            .expect("one JSON object");
    }
}

use std::fmt;

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::values::{AudioCodec, G711Ulaw, OpaqueId, RealtimeValueError, ToolOutput};

/// A closed outbound client event sent to the OpenAI Realtime provider.
#[derive(Clone, PartialEq, Eq)]
pub enum RealtimeClientEvent {
    /// Updates the provider session configuration.
    SessionUpdate {
        event_id: Option<OpaqueId>,
        session: SessionUpdatePayload,
    },
    /// Appends opaque G.711 mu-law bytes to the input audio buffer.
    InputAudioBufferAppend {
        event_id: Option<OpaqueId>,
        audio: G711Ulaw,
    },
    /// Commits the input audio buffer.
    InputAudioBufferCommit { event_id: Option<OpaqueId> },
    /// Clears the input audio buffer.
    InputAudioBufferClear { event_id: Option<OpaqueId> },
    /// Cancels the current response.
    ResponseCancel { event_id: Option<OpaqueId> },
    /// Creates a function-call output acknowledgement item.
    ConversationItemCreate {
        event_id: Option<OpaqueId>,
        item: FunctionCallOutputItem,
    },
}

/// The exact session update payload accepted by the client event boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionUpdatePayload {
    /// The model name, preserved without interpretation.
    pub model: String,
    /// The input audio codec.
    pub input_audio_format: AudioCodec,
    /// The output audio codec.
    pub output_audio_format: AudioCodec,
    /// Optional server-side voice activity detection mode.
    pub turn_detection: Option<TurnDetection>,
}

/// The supported Realtime turn-detection modes.
#[derive(Clone, PartialEq, Eq)]
pub struct TurnDetection {
    /// The exact wire value of the turn-detection mode.
    r#type: String,
}

impl TurnDetection {
    /// Constructs one of the two supported turn-detection modes.
    pub fn new(value: impl Into<String>) -> Result<Self, RealtimeValueError> {
        let value = value.into();
        match value.as_str() {
            "server_vad" | "semantic_vad" => Ok(Self { r#type: value }),
            _ => Err(RealtimeValueError::InvalidJson),
        }
    }
}

/// The sole function-call output item type accepted by the client boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCallOutputType {
    /// A function-call output acknowledgement.
    FunctionCallOutput,
}

/// A function-call output acknowledgement carried by a conversation item.
#[derive(Clone, PartialEq, Eq)]
pub struct FunctionCallOutputItem {
    /// Optional provider item identifier.
    pub id: Option<OpaqueId>,
    /// The closed function-call output item type.
    pub r#type: FunctionCallOutputType,
    /// The function call identifier being acknowledged.
    pub call_id: OpaqueId,
    /// Opaque tool output, preserved without parsing or execution.
    pub output: ToolOutput,
}

impl fmt::Debug for RealtimeClientEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealtimeClientEvent(<redacted>)")
    }
}

impl fmt::Debug for SessionUpdatePayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionUpdatePayload(<redacted>)")
    }
}

impl fmt::Debug for TurnDetection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TurnDetection(<redacted>)")
    }
}

impl fmt::Debug for FunctionCallOutputItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FunctionCallOutputItem(<redacted>)")
    }
}

#[derive(Serialize)]
struct SessionUpdateWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<&'a OpaqueId>,
    session: &'a SessionUpdatePayload,
}

#[derive(Serialize)]
struct InputAudioBufferAppendWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<&'a OpaqueId>,
    audio: &'a G711Ulaw,
}

#[derive(Serialize)]
struct EventIdWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<&'a OpaqueId>,
}

#[derive(Serialize)]
struct ConversationItemCreateWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<&'a OpaqueId>,
    item: &'a FunctionCallOutputItem,
}

#[derive(Serialize)]
struct SessionUpdatePayloadWire<'a> {
    model: &'a str,
    input_audio_format: AudioCodec,
    output_audio_format: AudioCodec,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_detection: Option<&'a TurnDetection>,
}

#[derive(Serialize)]
struct TurnDetectionWire<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
}

#[derive(Serialize)]
struct FunctionCallOutputItemWire<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a OpaqueId>,
    #[serde(rename = "type")]
    item_type: &'a FunctionCallOutputType,
    call_id: &'a OpaqueId,
    output: &'a ToolOutput,
}

impl Serialize for RealtimeClientEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::SessionUpdate { event_id, session } => SessionUpdateWire {
                kind: "session.update",
                event_id: event_id.as_ref(),
                session,
            }
            .serialize(serializer),
            Self::InputAudioBufferAppend { event_id, audio } => InputAudioBufferAppendWire {
                kind: "input_audio_buffer.append",
                event_id: event_id.as_ref(),
                audio,
            }
            .serialize(serializer),
            Self::InputAudioBufferCommit { event_id } => EventIdWire {
                kind: "input_audio_buffer.commit",
                event_id: event_id.as_ref(),
            }
            .serialize(serializer),
            Self::InputAudioBufferClear { event_id } => EventIdWire {
                kind: "input_audio_buffer.clear",
                event_id: event_id.as_ref(),
            }
            .serialize(serializer),
            Self::ResponseCancel { event_id } => EventIdWire {
                kind: "response.cancel",
                event_id: event_id.as_ref(),
            }
            .serialize(serializer),
            Self::ConversationItemCreate { event_id, item } => ConversationItemCreateWire {
                kind: "conversation.item.create",
                event_id: event_id.as_ref(),
                item,
            }
            .serialize(serializer),
        }
    }
}

impl Serialize for SessionUpdatePayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SessionUpdatePayloadWire {
            model: &self.model,
            input_audio_format: self.input_audio_format,
            output_audio_format: self.output_audio_format,
            turn_detection: self.turn_detection.as_ref(),
        }
        .serialize(serializer)
    }
}

impl Serialize for TurnDetection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        TurnDetectionWire { kind: &self.r#type }.serialize(serializer)
    }
}

impl Serialize for FunctionCallOutputType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("function_call_output")
    }
}

impl Serialize for FunctionCallOutputItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        FunctionCallOutputItemWire {
            id: self.id.as_ref(),
            item_type: &self.r#type,
            call_id: &self.call_id,
            output: &self.output,
        }
        .serialize(serializer)
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

fn optional(value: Option<Value>) -> Option<Value> {
    match value {
        None | Some(Value::Null) => None,
        Some(value) => Some(value),
    }
}

fn finish(map: Map<String, Value>) -> Result<(), RealtimeValueError> {
    if map.is_empty() {
        Ok(())
    } else {
        Err(RealtimeValueError::InvalidJson)
    }
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

fn optional_opaque_id(value: Option<Value>) -> Result<Option<OpaqueId>, RealtimeValueError> {
    optional(value).map(opaque_id).transpose()
}

fn codec(value: Value) -> Result<AudioCodec, RealtimeValueError> {
    match value {
        Value::String(value) => AudioCodec::try_from(value.as_str())
            .map_err(|_| RealtimeValueError::UnsupportedAudioFormat),
        _ => Err(RealtimeValueError::UnsupportedAudioFormat),
    }
}

fn audio(value: Value) -> Result<G711Ulaw, RealtimeValueError> {
    if !matches!(value, Value::String(_)) {
        return Err(RealtimeValueError::InvalidBase64);
    }
    serde_json::from_value::<G711Ulaw>(value).map_err(|error| {
        let message = error.to_string();
        if message == RealtimeValueError::AudioTooLarge.to_string() {
            RealtimeValueError::AudioTooLarge
        } else if message == RealtimeValueError::EmptyAudio.to_string() {
            RealtimeValueError::EmptyAudio
        } else {
            RealtimeValueError::InvalidBase64
        }
    })
}

fn tool_output(value: Value) -> Result<ToolOutput, RealtimeValueError> {
    match value {
        Value::String(value) => ToolOutput::new(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn parse_turn_detection(value: Value) -> Result<TurnDetection, RealtimeValueError> {
    let mut map = object(value)?;
    let kind = string(required(&mut map, "type")?)?;
    finish(map)?;
    TurnDetection::new(kind)
}

fn parse_session(value: Value) -> Result<SessionUpdatePayload, RealtimeValueError> {
    let mut map = object(value)?;
    let model = string(required(&mut map, "model")?)?;
    let input_audio_format = codec(required(&mut map, "input_audio_format")?)?;
    let output_audio_format = codec(required(&mut map, "output_audio_format")?)?;
    let turn_detection = optional(map.remove("turn_detection"))
        .map(parse_turn_detection)
        .transpose()?;
    finish(map)?;
    Ok(SessionUpdatePayload {
        model,
        input_audio_format,
        output_audio_format,
        turn_detection,
    })
}

fn parse_item_type(value: Value) -> Result<FunctionCallOutputType, RealtimeValueError> {
    match value {
        Value::String(value) if value == "function_call_output" => {
            Ok(FunctionCallOutputType::FunctionCallOutput)
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

fn parse_item(value: Value) -> Result<FunctionCallOutputItem, RealtimeValueError> {
    let mut map = object(value)?;
    let id = optional_opaque_id(map.remove("id"))?;
    let r#type = parse_item_type(required(&mut map, "type")?)?;
    let call_id = opaque_id(required(&mut map, "call_id")?)?;
    let output = tool_output(required(&mut map, "output")?)?;
    finish(map)?;
    Ok(FunctionCallOutputItem {
        id,
        r#type,
        call_id,
        output,
    })
}

fn parse_event(value: Value) -> Result<RealtimeClientEvent, RealtimeValueError> {
    let mut map = object(value)?;
    let kind = match map.remove("type") {
        Some(Value::String(value)) => value,
        _ => return Err(RealtimeValueError::UnknownEventType),
    };
    match kind.as_str() {
        "session.update" => {
            let event_id = optional_opaque_id(map.remove("event_id"))?;
            let session = parse_session(required(&mut map, "session")?)?;
            finish(map)?;
            Ok(RealtimeClientEvent::SessionUpdate { event_id, session })
        }
        "input_audio_buffer.append" => {
            let event_id = optional_opaque_id(map.remove("event_id"))?;
            let audio = audio(required(&mut map, "audio")?)?;
            finish(map)?;
            Ok(RealtimeClientEvent::InputAudioBufferAppend { event_id, audio })
        }
        "input_audio_buffer.commit" => {
            let event_id = optional_opaque_id(map.remove("event_id"))?;
            finish(map)?;
            Ok(RealtimeClientEvent::InputAudioBufferCommit { event_id })
        }
        "input_audio_buffer.clear" => {
            let event_id = optional_opaque_id(map.remove("event_id"))?;
            finish(map)?;
            Ok(RealtimeClientEvent::InputAudioBufferClear { event_id })
        }
        "response.cancel" => {
            let event_id = optional_opaque_id(map.remove("event_id"))?;
            finish(map)?;
            Ok(RealtimeClientEvent::ResponseCancel { event_id })
        }
        "conversation.item.create" => {
            let event_id = optional_opaque_id(map.remove("event_id"))?;
            let item = parse_item(required(&mut map, "item")?)?;
            finish(map)?;
            Ok(RealtimeClientEvent::ConversationItemCreate { event_id, item })
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

impl<'de> Deserialize<'de> for RealtimeClientEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_event(value).map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for SessionUpdatePayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_session(value).map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for TurnDetection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_turn_detection(value).map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for FunctionCallOutputType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_item_type(value).map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for FunctionCallOutputItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_item(value).map_err(D::Error::custom)
    }
}
