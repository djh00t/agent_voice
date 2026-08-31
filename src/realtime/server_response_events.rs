#[cfg(test)]
mod tests {
    use super::super::values::{OpaqueId, RealtimeValueError};
    use super::{
        InterruptionReason, ProviderErrorSummary, RealtimeServerResponseEvent, ResponseStatus,
        ResponseStatusDetails, ResponseSummary,
    };
    use serde_json::{Value, json};

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).expect("valid opaque id")
    }

    fn response(
        status: ResponseStatus,
        reason: Option<InterruptionReason>,
    ) -> RealtimeServerResponseEvent {
        RealtimeServerResponseEvent::ResponseDone {
            event_id: id("event-1"),
            response: ResponseSummary {
                id: id("response-1"),
                status,
                status_details: Some(ResponseStatusDetails {
                    reason,
                    error: Some(ProviderErrorSummary {
                        r#type: "provider_error".to_owned(),
                        code: Some("E-42".to_owned()),
                    }),
                }),
            },
        }
    }

    fn rejected(raw: &str, expected: &str, secret: &str) -> String {
        let error = match serde_json::from_str::<RealtimeServerResponseEvent>(raw) {
            Ok(_) => panic!("fixture must be rejected: {raw}"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains(expected),
            "expected {expected:?} in {error:?}"
        );
        assert!(
            !error.contains(secret),
            "error leaked rejected payload: {error}"
        );
        error
    }

    #[test]
    fn response_done_interruptions() {
        let statuses = [
            (ResponseStatus::InProgress, "in_progress"),
            (ResponseStatus::Completed, "completed"),
            (ResponseStatus::Cancelled, "cancelled"),
            (ResponseStatus::Failed, "failed"),
            (ResponseStatus::Incomplete, "incomplete"),
        ];
        for (status, wire_status) in statuses {
            let reasons = if status == ResponseStatus::Cancelled {
                [
                    Some(InterruptionReason::TurnDetected),
                    Some(InterruptionReason::ClientCancelled),
                ]
            } else {
                [None, None]
            };
            for reason in reasons {
                let event = response(status, reason);
                let encoded = serde_json::to_value(&event).expect("serialize response.done");
                assert_eq!(encoded["type"], "response.done");
                assert_eq!(encoded["event_id"], "event-1");
                assert_eq!(encoded["response"]["id"], "response-1");
                assert_eq!(encoded["response"]["status"], wire_status);
                assert_eq!(
                    encoded["response"]["status_details"]["error"]["type"],
                    "provider_error"
                );
                assert_eq!(
                    encoded["response"]["status_details"]["error"]["code"],
                    "E-42"
                );
                assert_eq!(
                    encoded["response"]["status_details"]["reason"],
                    match reason {
                        Some(InterruptionReason::TurnDetected) => json!("turn_detected"),
                        Some(InterruptionReason::ClientCancelled) => json!("client_cancelled"),
                        None => Value::Null,
                    }
                );
                assert_eq!(
                    serde_json::from_value::<RealtimeServerResponseEvent>(encoded)
                        .expect("response.done round trip"),
                    event
                );
            }
        }

        let without_details = json!({
            "type": "response.done",
            "event_id": "event-1",
            "response": {
                "id": "response-1",
                "status": "completed",
                "status_details": null
            }
        });
        let decoded = serde_json::from_value::<RealtimeServerResponseEvent>(without_details)
            .expect("null optional status details");
        assert_eq!(
            serde_json::to_value(decoded).expect("serialize optional status details"),
            json!({
                "type": "response.done",
                "event_id": "event-1",
                "response": {
                    "id": "response-1",
                    "status": "completed",
                    "status_details": null
                }
            })
        );

        for (status, status_wire) in [
            ("completed", ResponseStatus::Completed),
            ("failed", ResponseStatus::Failed),
            ("in_progress", ResponseStatus::InProgress),
            ("incomplete", ResponseStatus::Incomplete),
        ] {
            let raw = format!(
                r#"{{"type":"response.done","event_id":"event-1","response":{{"id":"response-1","status":"{status}","status_details":{{"reason":"turn_detected","error":null}}}}}}"#
            );
            let error = rejected(&raw, "invalid interruption reason", "turn_detected");
            assert_eq!(
                error,
                RealtimeValueError::InvalidInterruptionReason.to_string()
            );
            assert_ne!(status_wire, ResponseStatus::Cancelled);
        }

        rejected(
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"cancelled","status_details":{"reason":"secret_reason","error":null}}}"#,
            "invalid interruption reason",
            "secret_reason",
        );
        rejected(
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"secret_status","status_details":null}}"#,
            "invalid response status",
            "secret_status",
        );

        for raw in [
            r#"{"type":"response.done","response":{"id":"response-1","status":"completed","status_details":null}}"#,
            r#"{"type":"response.done","event_id":null,"response":{"id":"response-1","status":"completed","status_details":null}}"#,
            r#"{"type":"response.done","event_id":"event-1"}"#,
            r#"{"type":"response.done","event_id":"event-1","response":null}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"status":"completed","status_details":null}}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":null,"status":"completed","status_details":null}}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status_details":null}}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":null,"status_details":null}}"#,
        ] {
            let error = serde_json::from_str::<RealtimeServerResponseEvent>(raw)
                .expect_err("required field must be rejected")
                .to_string();
            assert!(
                error.contains("missing required field"),
                "unexpected required-field error: {error}"
            );
        }

        for raw in [
            r#"{}"#,
            r#"{"type":null}"#,
            r#"{"type":"response.create"}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"completed","status_details":null},"secret":"payload-secret"}"#,
        ] {
            let expected = if raw.contains("secret") {
                "invalid JSON"
            } else {
                "unknown event type"
            };
            rejected(raw, expected, "payload-secret");
        }

        for raw in [
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"completed","status_details":{"reason":null,"error":{"type":"provider-secret","code":"secret-code","unexpected":"payload-secret"}}}}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"completed","status_details":{"reason":null,"error":{"type":"provider-secret","unexpected":"payload-secret"}}}}"#,
            r#"{"type":"response.done","event_id":"event-1","response":{"id":"response-1","status":"completed","unexpected":"payload-secret","status_details":null}}"#,
        ] {
            rejected(raw, "invalid JSON", "payload-secret");
        }

        let malformed = serde_json::from_str::<RealtimeServerResponseEvent>(
            r#"{"type":"response.done","event_id":"event-secret""#,
        )
        .expect_err("malformed JSON must be rejected")
        .to_string();
        assert!(!malformed.contains("event-secret"));

        let debug = format!(
            "{:?}",
            response(
                ResponseStatus::Cancelled,
                Some(InterruptionReason::ClientCancelled)
            )
        );
        for secret in ["event-1", "response-1", "provider_error", "E-42"] {
            assert!(!debug.contains(secret), "debug leaked payload: {debug}");
        }
    }
}

use std::fmt;

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::values::{OpaqueId, RealtimeValueError};

/// The closed set of provider response completion statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    /// The response is still being generated.
    InProgress,
    /// The response completed normally.
    Completed,
    /// The response was cancelled or interrupted.
    Cancelled,
    /// The provider failed to complete the response.
    Failed,
    /// The response ended without completing all requested work.
    Incomplete,
}

impl ResponseStatus {
    /// Returns the exact snake_case wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
        }
    }
}

impl TryFrom<&str> for ResponseStatus {
    type Error = RealtimeValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "incomplete" => Ok(Self::Incomplete),
            _ => Err(RealtimeValueError::InvalidResponseStatus),
        }
    }
}

impl TryFrom<String> for ResponseStatus {
    type Error = RealtimeValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl Serialize for ResponseStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ResponseStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidResponseStatus))?;
        Self::try_from(value).map_err(D::Error::custom)
    }
}

/// The closed set of reasons for an interrupted response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptionReason {
    /// The provider detected a new turn while generating the response.
    TurnDetected,
    /// The client explicitly cancelled the response.
    ClientCancelled,
}

impl InterruptionReason {
    /// Returns the exact snake_case wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TurnDetected => "turn_detected",
            Self::ClientCancelled => "client_cancelled",
        }
    }
}

impl TryFrom<&str> for InterruptionReason {
    type Error = RealtimeValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "turn_detected" => Ok(Self::TurnDetected),
            "client_cancelled" => Ok(Self::ClientCancelled),
            _ => Err(RealtimeValueError::InvalidInterruptionReason),
        }
    }
}

impl TryFrom<String> for InterruptionReason {
    type Error = RealtimeValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl Serialize for InterruptionReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for InterruptionReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(RealtimeValueError::InvalidInterruptionReason))?;
        Self::try_from(value).map_err(D::Error::custom)
    }
}

/// Optional provider error detail carried by a response status.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ProviderErrorSummary {
    /// The provider's error category.
    #[serde(rename = "type")]
    pub r#type: String,
    /// The optional provider error code.
    pub code: Option<String>,
}

impl fmt::Debug for ProviderErrorSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderErrorSummary(<redacted>)")
    }
}

/// Optional response status details.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ResponseStatusDetails {
    /// The optional interruption reason.
    pub reason: Option<InterruptionReason>,
    /// The optional provider error summary.
    pub error: Option<ProviderErrorSummary>,
}

impl fmt::Debug for ResponseStatusDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseStatusDetails(<redacted>)")
    }
}

/// The bounded response metadata carried by `response.done`.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ResponseSummary {
    /// The required provider response identifier.
    pub id: OpaqueId,
    /// The closed response completion status.
    pub status: ResponseStatus,
    /// Optional interruption or provider error details.
    pub status_details: Option<ResponseStatusDetails>,
}

impl fmt::Debug for ResponseSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseSummary(<redacted>)")
    }
}

/// A typed `response.done` payload without its event tag.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ResponseDone {
    /// The required provider event identifier.
    pub event_id: OpaqueId,
    /// The completed response metadata.
    pub response: ResponseSummary,
}

impl fmt::Debug for ResponseDone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseDone(<redacted>)")
    }
}

/// The closed server response event set owned by this package.
#[derive(Clone, PartialEq, Eq)]
pub enum RealtimeServerResponseEvent {
    /// Indicates that a response completed, failed, was interrupted, or is incomplete.
    ResponseDone {
        /// The required provider event identifier.
        event_id: OpaqueId,
        /// The completed response metadata.
        response: ResponseSummary,
    },
}

impl fmt::Debug for RealtimeServerResponseEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealtimeServerResponseEvent(<redacted>)")
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

fn parse_provider_error(value: Value) -> Result<ProviderErrorSummary, RealtimeValueError> {
    let mut map = object(value)?;
    let r#type = string(required(&mut map, "type")?)?;
    let code = optional(map.remove("code")).map(string).transpose()?;
    finish(map)?;
    Ok(ProviderErrorSummary { r#type, code })
}

fn parse_status_details(value: Value) -> Result<ResponseStatusDetails, RealtimeValueError> {
    let mut map = object(value)?;
    let reason = optional(map.remove("reason"))
        .map(parse_reason)
        .transpose()?;
    let error = optional(map.remove("error"))
        .map(parse_provider_error)
        .transpose()?;
    finish(map)?;
    Ok(ResponseStatusDetails { reason, error })
}

fn parse_response_summary(value: Value) -> Result<ResponseSummary, RealtimeValueError> {
    let mut map = object(value)?;
    let id = opaque_id(required(&mut map, "id")?)?;
    let status = parse_status(required(&mut map, "status")?)?;
    let status_details = optional(map.remove("status_details"))
        .map(parse_status_details)
        .transpose()?;
    finish(map)?;
    if status != ResponseStatus::Cancelled
        && status_details
            .as_ref()
            .and_then(|details| details.reason)
            .is_some()
    {
        return Err(RealtimeValueError::InvalidInterruptionReason);
    }
    Ok(ResponseSummary {
        id,
        status,
        status_details,
    })
}

fn parse_response_done_payload(value: Value) -> Result<ResponseDone, RealtimeValueError> {
    let mut map = object(value)?;
    let event_id = opaque_id(required(&mut map, "event_id")?)?;
    let response = parse_response_summary(required(&mut map, "response")?)?;
    finish(map)?;
    Ok(ResponseDone { event_id, response })
}

fn parse_event(value: Value) -> Result<RealtimeServerResponseEvent, RealtimeValueError> {
    let mut map = object(value)?;
    let kind = match map.remove("type") {
        Some(Value::String(value)) => value,
        _ => return Err(RealtimeValueError::UnknownEventType),
    };
    match kind.as_str() {
        "response.done" => {
            let event_id = opaque_id(required(&mut map, "event_id")?)?;
            let response = parse_response_summary(required(&mut map, "response")?)?;
            finish(map)?;
            Ok(RealtimeServerResponseEvent::ResponseDone { event_id, response })
        }
        _ => Err(RealtimeValueError::UnknownEventType),
    }
}

impl<'de> Deserialize<'de> for ProviderErrorSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_provider_error(
            Value::deserialize(deserializer)
                .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?,
        )
        .map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ResponseStatusDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_status_details(
            Value::deserialize(deserializer)
                .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?,
        )
        .map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ResponseSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_response_summary(
            Value::deserialize(deserializer)
                .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?,
        )
        .map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ResponseDone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_response_done_payload(
            Value::deserialize(deserializer)
                .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Serialize)]
struct ResponseDoneWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    event_id: &'a OpaqueId,
    response: &'a ResponseSummary,
}

impl Serialize for RealtimeServerResponseEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::ResponseDone { event_id, response } => ResponseDoneWire {
                kind: "response.done",
                event_id,
                response,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RealtimeServerResponseEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_event(
            Value::deserialize(deserializer)
                .map_err(|_| D::Error::custom(RealtimeValueError::InvalidJson))?,
        )
        .map_err(D::Error::custom)
    }
}
