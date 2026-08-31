#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
    use serde_json::{json, Value};

    use super::super::values::{G711Ulaw, OpaqueId, RealtimeValueError, TranscriptText};
    use super::RealtimeServerAudioEvent;

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).expect("test identifier is valid")
    }

    fn audio() -> G711Ulaw {
        G711Ulaw::new(vec![0, 1, 2, 250]).expect("test audio is valid")
    }

    fn transcript() -> TranscriptText {
        TranscriptText::new("Apt 4B, call 2").expect("test transcript is valid")
    }

    fn redacted_error(raw: &str, secret: &str) -> String {
        let error = serde_json::from_str::<RealtimeServerAudioEvent>(raw)
            .expect_err("fixture must be rejected")
            .to_string();
        assert!(
            !error.contains(secret),
            "rejected payload leaked in error: {error}"
        );
        error
    }

    #[test]
    fn output_audio_events() {
        let event_id = id("event-1");
        let response_id = id("response-1");
        let item_id = id("item-1");

        let delta = RealtimeServerAudioEvent::OutputAudioDelta {
            event_id: event_id.clone(),
            response_id: response_id.clone(),
            item_id: item_id.clone(),
            output_index: 1,
            content_index: 2,
            delta: audio(),
        };
        let delta_json = json!({
            "type": "response.output_audio.delta",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2,
            "delta": "AAEC+g=="
        });
        assert_eq!(
            serde_json::to_value(&delta).expect("serialize delta"),
            delta_json
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerAudioEvent>(delta_json.clone())
                .expect("deserialize delta"),
            delta
        );

        let done = RealtimeServerAudioEvent::OutputAudioDone {
            event_id: event_id.clone(),
            response_id: response_id.clone(),
            item_id: item_id.clone(),
            output_index: 1,
            content_index: 2,
        };
        let done_json = json!({
            "type": "response.output_audio.done",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2
        });
        assert_eq!(
            serde_json::to_value(&done).expect("serialize done"),
            done_json
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerAudioEvent>(done_json.clone())
                .expect("deserialize done"),
            done
        );

        let transcript_delta = RealtimeServerAudioEvent::OutputAudioTranscriptDelta {
            event_id: event_id.clone(),
            response_id: response_id.clone(),
            item_id: item_id.clone(),
            output_index: 1,
            content_index: 2,
            delta: transcript(),
        };
        let transcript_delta_json = json!({
            "type": "response.output_audio_transcript.delta",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2,
            "delta": "Apt 4B, call 2"
        });
        assert_eq!(
            serde_json::to_value(&transcript_delta).expect("serialize transcript delta"),
            transcript_delta_json
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerAudioEvent>(transcript_delta_json.clone())
                .expect("deserialize transcript delta"),
            transcript_delta
        );

        let transcript_done = RealtimeServerAudioEvent::OutputAudioTranscriptDone {
            event_id,
            response_id,
            item_id,
            output_index: 1,
            content_index: 2,
            transcript: transcript(),
        };
        let transcript_done_json = json!({
            "type": "response.output_audio_transcript.done",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2,
            "transcript": "Apt 4B, call 2"
        });
        assert_eq!(
            serde_json::to_value(&transcript_done).expect("serialize transcript done"),
            transcript_done_json
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerAudioEvent>(transcript_done_json)
                .expect("deserialize transcript done"),
            transcript_done
        );

        let debug = format!("{delta:?}");
        for secret in ["event-1", "response-1", "item-1", "AAEC+g=="] {
            assert!(!debug.contains(secret), "debug leaked secret: {debug}");
        }
        let transcript_debug = format!("{transcript_done:?}");
        assert!(!transcript_debug.contains("Apt 4B, call 2"));

        for raw in [
            r#"{"type":"response.audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g=="}"#,
            r#"{"type":"response.audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            r#"{"type":"response.output_audio.other","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            r#"{"type":"response.audio.delta"}"#,
        ] {
            let error = redacted_error(raw, "response.audio");
            assert!(error.contains("unknown event type"));
        }

        for raw in [
            r#"{"event_id":"event-1"}"#,
            r#"{"type":null,"event_id":"event-1"}"#,
            r#"{"type":42,"event_id":"event-1"}"#,
            r#"[]"#,
            r#"null"#,
            r#""not an event""#,
        ] {
            let error = redacted_error(raw, "event-1");
            assert!(error.contains("unknown event type") || error.contains("invalid JSON"));
        }

        let complete_done = json!({
            "type": "response.output_audio.done",
            "event_id": "event-1",
            "response_id": "response-1",
            "item_id": "item-1",
            "output_index": 1,
            "content_index": 2
        });
        for (field, secret) in [
            ("event_id", "event-1"),
            ("response_id", "response-1"),
            ("item_id", "item-1"),
            ("output_index", "1"),
            ("content_index", "2"),
        ] {
            let mut missing = complete_done.clone();
            missing.as_object_mut().expect("event object").remove(field);
            let missing_raw = serde_json::to_string(&missing).expect("missing field JSON");
            let error = redacted_error(&missing_raw, secret);
            assert!(error.contains("missing required field"));

            let mut null = complete_done.clone();
            null.as_object_mut()
                .expect("event object")
                .insert(field.to_owned(), Value::Null);
            let null_raw = serde_json::to_string(&null).expect("null field JSON");
            let error = redacted_error(&null_raw, secret);
            assert!(error.contains("missing required field"));
        }

        for raw in [
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":""}"#,
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"not-base64"}"#,
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g"}"#,
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g==\n"}"#,
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"-w=="}"#,
        ] {
            let error = redacted_error(raw, "delta");
            assert!(error.contains("audio is empty") || error.contains("invalid base64 audio"));
        }

        let oversized_audio = BASE64_STANDARD.encode(vec![0x2a; 16_385]);
        let oversized_raw = format!(
            "{{\"type\":\"response.output_audio.delta\",\"event_id\":\"event-1\",\"response_id\":\"response-1\",\"item_id\":\"item-1\",\"output_index\":1,\"content_index\":2,\"delta\":\"{oversized_audio}\"}}"
        );
        let error = redacted_error(&oversized_raw, &oversized_audio[..16]);
        assert!(error.contains("audio is too large"));

        let oversized_transcript = "x".repeat(4_097);
        let oversized_transcript_raw = format!(
            "{{\"type\":\"response.output_audio_transcript.done\",\"event_id\":\"event-1\",\"response_id\":\"response-1\",\"item_id\":\"item-1\",\"output_index\":1,\"content_index\":2,\"transcript\":\"{oversized_transcript}\"}}"
        );
        let error = redacted_error(&oversized_transcript_raw, &oversized_transcript[..16]);
        assert!(error.contains("transcript is too long"));

        for raw in [
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g==","secret":"payload-secret"}"#,
            r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"secret":"payload-secret"}"#,
            r#"{"type":"response.output_audio_transcript.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"safe","secret":"payload-secret"}"#,
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"transcript":"safe","secret":"payload-secret"}"#,
        ] {
            let error = redacted_error(raw, "payload-secret");
            assert!(error.contains("invalid JSON"));
        }

        for raw in [
            r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g=="}"#,
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"safe","transcript":"safe"}"#,
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"transcript":"safe","delta":"AAEC+g=="}"#,
        ] {
            let error = redacted_error(raw, "safe");
            assert!(error.contains("invalid JSON"));
        }

        for raw in [
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":null}"#,
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"transcript":null}"#,
        ] {
            let error = redacted_error(raw, "event-1");
            assert!(error.contains("missing required field"));
        }

        for (raw, secret) in [
            (
                r#"{"type":"response.audio.done","type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
                "response.audio.done",
            ),
            (
                r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","response_id":"response-2","item_id":"item-1","output_index":1,"content_index":2}"#,
                "response-2",
            ),
            (
                r#"{"type":"response.output_audio.done","event_id":"event-1","event_id":"event-2","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2}"#,
                "event-2",
            ),
            (
                r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","item_id":"item-2","output_index":1,"content_index":2}"#,
                "item-2",
            ),
            (
                r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"output_index":2,"content_index":2}"#,
                "2",
            ),
            (
                r#"{"type":"response.output_audio.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"content_index":3}"#,
                "3",
            ),
            (
                r#"{"type":"response.output_audio.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g==","delta":"AQID"}"#,
                "AQID",
            ),
            (
                r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"transcript":"draft","transcript":"final"}"#,
                "final",
            ),
        ] {
            let error = redacted_error(raw, secret);
            assert!(error.contains("invalid JSON"));
        }

        for raw in [
            r#"{"type":"response.output_audio.delta","event_id":"bad id","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"delta":"AAEC+g=="}"#,
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":-1,"content_index":2,"transcript":"safe"}"#,
            r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":"2","transcript":"safe"}"#,
        ] {
            let error = redacted_error(raw, "bad id");
            assert!(!error.contains('{'));
            assert!(error.contains("invalid opaque identifier") || error.contains("invalid JSON"));
        }

        let _: Value = serde_json::from_str(&serde_json::to_string(&delta).expect("JSON object"))
            .expect("serialized event is one JSON object");

        assert_eq!(
            serde_json::from_str::<RealtimeServerAudioEvent>(
                r#"{"type":"response.output_audio_transcript.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":1,"content_index":2,"transcript":"Apt 4B, call 2"}"#
            )
            .expect("lossless transcript"),
            transcript_done
        );
        assert_eq!(
            RealtimeValueError::UnknownEventType.to_string(),
            "unknown event type"
        );
    }
}

use std::fmt;

use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::values::{G711Ulaw, OpaqueId, RealtimeValueError, TranscriptText};

/// Closed server events carrying generated audio and its transcript.
#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum RealtimeServerAudioEvent {
    /// One generated G.711 mu-law audio delta.
    OutputAudioDelta {
        /// Provider event identifier.
        event_id: OpaqueId,
        /// Response identifier.
        response_id: OpaqueId,
        /// Output item identifier.
        item_id: OpaqueId,
        /// Zero-based output index.
        output_index: u32,
        /// Zero-based content index.
        content_index: u32,
        /// Opaque generated audio bytes.
        delta: G711Ulaw,
    },
    /// Completion of one generated audio item.
    OutputAudioDone {
        /// Provider event identifier.
        event_id: OpaqueId,
        /// Response identifier.
        response_id: OpaqueId,
        /// Output item identifier.
        item_id: OpaqueId,
        /// Zero-based output index.
        output_index: u32,
        /// Zero-based content index.
        content_index: u32,
    },
    /// One generated transcript delta accompanying audio.
    OutputAudioTranscriptDelta {
        /// Provider event identifier.
        event_id: OpaqueId,
        /// Response identifier.
        response_id: OpaqueId,
        /// Output item identifier.
        item_id: OpaqueId,
        /// Zero-based output index.
        output_index: u32,
        /// Zero-based content index.
        content_index: u32,
        /// Lossless transcript fragment.
        delta: TranscriptText,
    },
    /// Completion of one generated transcript.
    OutputAudioTranscriptDone {
        /// Provider event identifier.
        event_id: OpaqueId,
        /// Response identifier.
        response_id: OpaqueId,
        /// Output item identifier.
        item_id: OpaqueId,
        /// Zero-based output index.
        output_index: u32,
        /// Zero-based content index.
        content_index: u32,
        /// Lossless completed transcript.
        transcript: TranscriptText,
    },
}

impl fmt::Debug for RealtimeServerAudioEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealtimeServerAudioEvent(<redacted>)")
    }
}

#[derive(Serialize)]
struct OutputAudioDeltaWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    event_id: &'a OpaqueId,
    response_id: &'a OpaqueId,
    item_id: &'a OpaqueId,
    output_index: u32,
    content_index: u32,
    delta: &'a G711Ulaw,
}

#[derive(Serialize)]
struct OutputAudioDoneWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    event_id: &'a OpaqueId,
    response_id: &'a OpaqueId,
    item_id: &'a OpaqueId,
    output_index: u32,
    content_index: u32,
}

#[derive(Serialize)]
struct OutputAudioTranscriptDeltaWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    event_id: &'a OpaqueId,
    response_id: &'a OpaqueId,
    item_id: &'a OpaqueId,
    output_index: u32,
    content_index: u32,
    delta: &'a TranscriptText,
}

#[derive(Serialize)]
struct OutputAudioTranscriptDoneWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    event_id: &'a OpaqueId,
    response_id: &'a OpaqueId,
    item_id: &'a OpaqueId,
    output_index: u32,
    content_index: u32,
    transcript: &'a TranscriptText,
}

impl Serialize for RealtimeServerAudioEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::OutputAudioDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            } => OutputAudioDeltaWire {
                kind: "response.output_audio.delta",
                event_id,
                response_id,
                item_id,
                output_index: *output_index,
                content_index: *content_index,
                delta,
            }
            .serialize(serializer),
            Self::OutputAudioDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
            } => OutputAudioDoneWire {
                kind: "response.output_audio.done",
                event_id,
                response_id,
                item_id,
                output_index: *output_index,
                content_index: *content_index,
            }
            .serialize(serializer),
            Self::OutputAudioTranscriptDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            } => OutputAudioTranscriptDeltaWire {
                kind: "response.output_audio_transcript.delta",
                event_id,
                response_id,
                item_id,
                output_index: *output_index,
                content_index: *content_index,
                delta,
            }
            .serialize(serializer),
            Self::OutputAudioTranscriptDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                transcript,
            } => OutputAudioTranscriptDoneWire {
                kind: "response.output_audio_transcript.done",
                event_id,
                response_id,
                item_id,
                output_index: *output_index,
                content_index: *content_index,
                transcript,
            }
            .serialize(serializer),
        }
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

struct UniqueValueSeed;

impl<'de> DeserializeSeed<'de> for UniqueValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
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
        D: Deserializer<'de>,
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

fn opaque_id(value: Value) -> Result<OpaqueId, RealtimeValueError> {
    match value {
        Value::String(value) => OpaqueId::new(value),
        _ => Err(RealtimeValueError::InvalidOpaqueId),
    }
}

fn index(value: Value) -> Result<u32, RealtimeValueError> {
    serde_json::from_value(value).map_err(|_| RealtimeValueError::InvalidJson)
}

fn audio(value: Value) -> Result<G711Ulaw, RealtimeValueError> {
    if !matches!(value, Value::String(_)) {
        return Err(RealtimeValueError::InvalidBase64);
    }
    serde_json::from_value::<G711Ulaw>(value).map_err(|error| {
        let message = error.to_string();
        if message.contains("audio is too large") {
            RealtimeValueError::AudioTooLarge
        } else if message.contains("audio is empty") {
            RealtimeValueError::EmptyAudio
        } else {
            RealtimeValueError::InvalidBase64
        }
    })
}

fn transcript(value: Value) -> Result<TranscriptText, RealtimeValueError> {
    match value {
        Value::String(value) => TranscriptText::new(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn parse_event(value: Value) -> Result<RealtimeServerAudioEvent, RealtimeValueError> {
    let mut map = object(value)?;
    let kind = match map.remove("type") {
        Some(Value::String(value)) => value,
        _ => return Err(RealtimeValueError::UnknownEventType),
    };

    match kind.as_str() {
        "response.output_audio.delta" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let delta = audio(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerAudioEvent::OutputAudioDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            })
        }
        "response.output_audio.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            finish(map)?;
            Ok(RealtimeServerAudioEvent::OutputAudioDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
            })
        }
        "response.output_audio_transcript.delta" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let delta = transcript(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerAudioEvent::OutputAudioTranscriptDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            })
        }
        "response.output_audio_transcript.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = index(required(&mut map, "output_index")?)?;
            let content_index = index(required(&mut map, "content_index")?)?;
            let transcript = transcript(required(&mut map, "transcript")?)?;
            finish(map)?;
            Ok(RealtimeServerAudioEvent::OutputAudioTranscriptDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                transcript,
            })
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

impl<'de> Deserialize<'de> for RealtimeServerAudioEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserializer
            .deserialize_any(UniqueValueVisitor)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_event(value).map_err(D::Error::custom)
    }
}
