#[cfg(test)]
mod tests {
    use super::super::values::{OpaqueId, TranscriptText};
    use super::{
        ProviderError, RealtimeServerSessionEvent, SessionInfo,
    };
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
        let completed = RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionCompleted {
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
            assert_eq!(error_for(unknown_field, &["payload-secret"]), "invalid JSON");
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
        assert_eq!(error_for(invalid_id, &["bad id"]), "invalid opaque identifier");
        let invalid_nested_id = r#"{"type":"input_audio_buffer.committed","event_id":"event-1","item_id":"bad id"}"#;
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
