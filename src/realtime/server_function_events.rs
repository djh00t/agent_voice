#[cfg(test)]
mod tests {
    use super::super::values::{FunctionArguments, OpaqueId, ToolOutput};
    use super::{FunctionCallOutputAckItem, FunctionCallOutputType, RealtimeServerFunctionEvent};
    use serde_json::{Value, json};

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).expect("valid opaque id")
    }

    fn error_for(raw: &str, rejected: &[&str]) -> String {
        let error = serde_json::from_str::<RealtimeServerFunctionEvent>(raw)
            .expect_err("fixture must be rejected")
            .to_string();
        for secret in rejected {
            assert!(!error.contains(secret), "error leaked fixture: {error}");
        }
        error
    }

    #[test]
    fn function_call_events() {
        let event_id = id("event-1");
        let response_id = id("response-1");
        let item_id = id("item-1");
        let call_id = id("call-1");

        let delta_source = r#"{"city":"Syd""#;
        let delta = FunctionArguments::from_delta(delta_source).expect("fragment is bounded");
        let delta_event = RealtimeServerFunctionEvent::FunctionCallArgumentsDelta {
            event_id: event_id.clone(),
            response_id: response_id.clone(),
            item_id: item_id.clone(),
            output_index: 2,
            call_id: call_id.clone(),
            delta,
        };
        let delta_json = serde_json::to_value(&delta_event).expect("serialize delta event");
        assert_eq!(
            delta_json,
            json!({
                "type": "response.function_call_arguments.delta",
                "event_id": "event-1",
                "response_id": "response-1",
                "item_id": "item-1",
                "output_index": 2,
                "call_id": "call-1",
                "delta": delta_source,
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerFunctionEvent>(delta_json.clone())
                .expect("round-trip delta event"),
            delta_event
        );

        let completed_source = " { \"city\": \"Sydney\" } ";
        let completed =
            FunctionArguments::from_completed(completed_source).expect("completed object");
        let done_event = RealtimeServerFunctionEvent::FunctionCallArgumentsDone {
            event_id: event_id.clone(),
            response_id: response_id.clone(),
            item_id: item_id.clone(),
            output_index: 2,
            call_id: call_id.clone(),
            name: "get_weather exact name".to_owned(),
            arguments: completed,
        };
        let done_json = serde_json::to_value(&done_event).expect("serialize done event");
        assert_eq!(
            done_json,
            json!({
                "type": "response.function_call_arguments.done",
                "event_id": "event-1",
                "response_id": "response-1",
                "item_id": "item-1",
                "output_index": 2,
                "call_id": "call-1",
                "name": "get_weather exact name",
                "arguments": completed_source,
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerFunctionEvent>(done_json)
                .expect("round-trip done event"),
            done_event
        );

        let ack = RealtimeServerFunctionEvent::ConversationItemCreated {
            event_id: event_id.clone(),
            item: FunctionCallOutputAckItem {
                id: item_id.clone(),
                r#type: FunctionCallOutputType::FunctionCallOutput,
                call_id: call_id.clone(),
                output: ToolOutput::new("provider output exact: café ✓").expect("bounded output"),
            },
        };
        let ack_json = serde_json::to_value(&ack).expect("serialize acknowledgement");
        assert_eq!(
            ack_json,
            json!({
                "type": "conversation.item.created",
                "event_id": "event-1",
                "item": {
                    "id": "item-1",
                    "type": "function_call_output",
                    "call_id": "call-1",
                    "output": "provider output exact: café ✓",
                },
            })
        );
        assert_eq!(
            serde_json::from_value::<RealtimeServerFunctionEvent>(ack_json)
                .expect("round-trip acknowledgement"),
            ack
        );

        assert!(FunctionArguments::from_delta("x".repeat(16_384)).is_ok());
        assert_eq!(
            FunctionArguments::from_delta("x".repeat(16_385)),
            Err(super::super::values::RealtimeValueError::ArgumentsTooLong)
        );
        for invalid in ["[]", "42", "null", "true", "{malformed"] {
            assert_eq!(
                FunctionArguments::from_completed(invalid),
                Err(super::super::values::RealtimeValueError::InvalidArgumentsJson)
            );
            let raw = format!(
                "{{\"type\":\"response.function_call_arguments.done\",\"event_id\":\"event-1\",\"response_id\":\"response-1\",\"item_id\":\"item-1\",\"output_index\":2,\"call_id\":\"call-1\",\"name\":\"secret-name\",\"arguments\":{}}}",
                serde_json::to_string(invalid).expect("JSON string")
            );
            let error = error_for(&raw, &[invalid, "secret-name"]);
            assert!(error.contains("function arguments"));
        }
        let oversized = "x".repeat(16_385);
        let oversized_json = serde_json::to_string(&oversized).expect("JSON string");
        let oversized_raw = format!(
            "{{\"type\":\"response.function_call_arguments.done\",\"event_id\":\"event-1\",\"response_id\":\"response-1\",\"item_id\":\"item-1\",\"output_index\":2,\"call_id\":\"call-1\",\"name\":\"secret-name\",\"arguments\":{oversized_json}}}"
        );
        assert_eq!(
            error_for(&oversized_raw, &["xxxxxxxx", "secret-name"]),
            "function arguments are too long"
        );

        let invalid_item_type = r#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":"function_call","call_id":"call-1","output":"secret-output"}}"#;
        assert_eq!(
            error_for(invalid_item_type, &["function_call", "secret-output"]),
            "unknown event type"
        );
        let invalid_type = r#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":null,"call_id":"call-1","output":"secret-output"}}"#;
        assert_eq!(
            error_for(invalid_type, &["secret-output"]),
            "missing required field"
        );

        for unknown_tag in [
            "response.function_call_arguments",
            "response.function_call_arguments.delta.v2",
            "conversation.item.create",
        ] {
            let raw = format!(r#"{{"type":"{unknown_tag}"}}"#);
            assert_eq!(error_for(&raw, &[unknown_tag]), "unknown event type");
        }
        for unknown_field in [
            r#"{"type":"response.function_call_arguments.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","delta":"{}","secret":"payload-secret"}"#,
            r#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":"function_call_output","call_id":"call-1","output":"safe","secret":"payload-secret"}}"#,
        ] {
            assert_eq!(
                error_for(unknown_field, &["payload-secret"]),
                "invalid JSON"
            );
        }

        for missing in [
            r#"{"type":"response.function_call_arguments.delta","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","delta":"{}"}"#,
            r#"{"type":"response.function_call_arguments.done","event_id":null,"response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","name":"safe","arguments":"{}"}"#,
            r#"{"type":"response.function_call_arguments.done","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","name":"safe"}"#,
            r#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":"function_call_output","output":"safe"}}"#,
        ] {
            let error = error_for(missing, &["safe"]);
            assert_eq!(error, "missing required field");
        }
        for invalid_id in [
            r#"{"type":"response.function_call_arguments.delta","event_id":"bad id","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","delta":"{}"}"#,
            r#"{"type":"conversation.item.created","event_id":"event-1","item":{"id":"item-1","type":"function_call_output","call_id":"bad id","output":"safe"}}"#,
        ] {
            assert_eq!(
                error_for(invalid_id, &["bad id"]),
                "invalid opaque identifier"
            );
        }
        let malformed = error_for(
            r#"{"type":"response.function_call_arguments.delta","event_id":"event-1","response_id":"response-1","item_id":"item-1","output_index":2,"call_id":"call-1","delta":"unterminated""#,
            &["unterminated"],
        );
        assert_eq!(malformed, "invalid JSON");

        let debug = format!("{:?}", done_event);
        for secret in [
            "event-1",
            "response-1",
            "item-1",
            "call-1",
            "get_weather exact name",
            completed_source,
        ] {
            assert!(!debug.contains(secret), "debug leaked secret: {debug}");
        }
        let _: Value = serde_json::from_value(delta_json).expect("one JSON object");
    }
}

use std::fmt;

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::values::{FunctionArguments, OpaqueId, RealtimeValueError, ToolOutput};

/// A closed server event carrying function-call arguments or an output acknowledgement.
#[derive(Clone, PartialEq, Eq)]
pub enum RealtimeServerFunctionEvent {
    /// A partial function-call argument fragment.
    FunctionCallArgumentsDelta {
        event_id: OpaqueId,
        response_id: OpaqueId,
        item_id: OpaqueId,
        output_index: u32,
        call_id: OpaqueId,
        delta: FunctionArguments,
    },
    /// A completed function-call argument object.
    FunctionCallArgumentsDone {
        event_id: OpaqueId,
        response_id: OpaqueId,
        item_id: OpaqueId,
        output_index: u32,
        call_id: OpaqueId,
        name: String,
        arguments: FunctionArguments,
    },
    /// A data-only acknowledgement for a function-call output item.
    ConversationItemCreated {
        event_id: OpaqueId,
        item: FunctionCallOutputAckItem,
    },
}

/// The closed item type used by a function-call output acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCallOutputType {
    /// The only accepted function-call output item type.
    FunctionCallOutput,
}

/// A function-call output acknowledgement item; its output is inert data.
#[derive(Clone, PartialEq, Eq)]
pub struct FunctionCallOutputAckItem {
    /// The provider item identifier.
    pub id: OpaqueId,
    /// The closed item type.
    pub r#type: FunctionCallOutputType,
    /// The function call identifier being acknowledged.
    pub call_id: OpaqueId,
    /// Opaque tool output that is never executed here.
    pub output: ToolOutput,
}

impl fmt::Debug for RealtimeServerFunctionEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealtimeServerFunctionEvent(<redacted>)")
    }
}

impl fmt::Debug for FunctionCallOutputAckItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FunctionCallOutputAckItem(<redacted>)")
    }
}

#[derive(Serialize)]
struct FunctionCallArgumentsDeltaWire<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    event_id: &'a OpaqueId,
    response_id: &'a OpaqueId,
    item_id: &'a OpaqueId,
    output_index: u32,
    call_id: &'a OpaqueId,
    delta: &'a FunctionArguments,
}

#[derive(Serialize)]
struct FunctionCallArgumentsDoneWire<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    event_id: &'a OpaqueId,
    response_id: &'a OpaqueId,
    item_id: &'a OpaqueId,
    output_index: u32,
    call_id: &'a OpaqueId,
    name: &'a str,
    arguments: &'a FunctionArguments,
}

#[derive(Serialize)]
struct ConversationItemCreatedWire<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    event_id: &'a OpaqueId,
    item: &'a FunctionCallOutputAckItem,
}

#[derive(Serialize)]
struct FunctionCallOutputAckItemWire<'a> {
    id: &'a OpaqueId,
    #[serde(rename = "type")]
    item_type: &'a FunctionCallOutputType,
    call_id: &'a OpaqueId,
    output: &'a ToolOutput,
}

impl Serialize for RealtimeServerFunctionEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::FunctionCallArgumentsDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                delta,
            } => FunctionCallArgumentsDeltaWire {
                event_type: "response.function_call_arguments.delta",
                event_id,
                response_id,
                item_id,
                output_index: *output_index,
                call_id,
                delta,
            }
            .serialize(serializer),
            Self::FunctionCallArgumentsDone {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                name,
                arguments,
            } => FunctionCallArgumentsDoneWire {
                event_type: "response.function_call_arguments.done",
                event_id,
                response_id,
                item_id,
                output_index: *output_index,
                call_id,
                name,
                arguments,
            }
            .serialize(serializer),
            Self::ConversationItemCreated { event_id, item } => ConversationItemCreatedWire {
                event_type: "conversation.item.created",
                event_id,
                item,
            }
            .serialize(serializer),
        }
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

impl Serialize for FunctionCallOutputAckItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        FunctionCallOutputAckItemWire {
            id: &self.id,
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

fn output_index(value: Value) -> Result<u32, RealtimeValueError> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(RealtimeValueError::InvalidJson)
}

fn delta(value: Value) -> Result<FunctionArguments, RealtimeValueError> {
    match value {
        Value::String(value) => FunctionArguments::from_delta(value),
        _ => Err(RealtimeValueError::InvalidJson),
    }
}

fn completed_arguments(value: Value) -> Result<FunctionArguments, RealtimeValueError> {
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

fn parse_item_type(value: Value) -> Result<FunctionCallOutputType, RealtimeValueError> {
    match value {
        Value::String(value) if value == "function_call_output" => {
            Ok(FunctionCallOutputType::FunctionCallOutput)
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

fn parse_item(value: Value) -> Result<FunctionCallOutputAckItem, RealtimeValueError> {
    let mut map = object(value)?;
    let id = opaque_id(required(&mut map, "id")?)?;
    let r#type = parse_item_type(required(&mut map, "type")?)?;
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

fn parse_event(value: Value) -> Result<RealtimeServerFunctionEvent, RealtimeValueError> {
    let mut map = object(value)?;
    let event_type = match map.remove("type") {
        Some(Value::String(value)) => value,
        _ => return Err(RealtimeValueError::UnknownEventType),
    };
    match event_type.as_str() {
        "response.function_call_arguments.delta" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = output_index(required(&mut map, "output_index")?)?;
            let call_id = opaque_id(required(&mut map, "call_id")?)?;
            let delta = delta(required(&mut map, "delta")?)?;
            finish(map)?;
            Ok(RealtimeServerFunctionEvent::FunctionCallArgumentsDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                delta,
            })
        }
        "response.function_call_arguments.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response_id = opaque_id(required(&mut map, "response_id")?)?;
            let item_id = opaque_id(required(&mut map, "item_id")?)?;
            let output_index = output_index(required(&mut map, "output_index")?)?;
            let call_id = opaque_id(required(&mut map, "call_id")?)?;
            let name = string(required(&mut map, "name")?)?;
            let arguments = completed_arguments(required(&mut map, "arguments")?)?;
            finish(map)?;
            Ok(RealtimeServerFunctionEvent::FunctionCallArgumentsDone {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                name,
                arguments,
            })
        }
        "conversation.item.created" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let item = parse_item(required(&mut map, "item")?)?;
            finish(map)?;
            Ok(RealtimeServerFunctionEvent::ConversationItemCreated { event_id, item })
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

impl<'de> Deserialize<'de> for RealtimeServerFunctionEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_event(value).map_err(D::Error::custom)
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

impl<'de> Deserialize<'de> for FunctionCallOutputAckItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?;
        parse_item(value).map_err(D::Error::custom)
    }
}
