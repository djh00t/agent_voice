#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
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
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::SessionCreated { .. }
                    )
                )
            },
        );
        assert_wire(
            r#"{"type":"session.updated","event_id":"event-updated","session":{"id":"session-2","model":"gpt-realtime-2026"}}"#,
            "session.updated",
            |event| {
                matches!(
                    event,
                    RealtimeServerEvent::Session(
                        RealtimeServerSessionEvent::SessionUpdated { .. }
                    )
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
                        RealtimeServerSessionEvent::ConversationItemInputAudioTranscriptionDelta {
                            ..
                        }
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
                    RealtimeServerEvent::Audio(
                        RealtimeServerAudioEvent::OutputAudioDelta { .. }
                    )
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
                    RealtimeServerEvent::Response(
                        RealtimeServerResponseEvent::ResponseDone { .. }
                    )
                )
            },
        );

        let client = RealtimeClientEvent::ResponseCancel {
            event_id: Some(id("client-event-1")),
        };
        let client_value = serde_json::to_value(&client).expect("serialize client smoke");
        assert_eq!(client_value, json!({"type":"response.cancel","event_id":"client-event-1"}));
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
            assert_eq!(decode_server_event(raw.as_bytes()), Err(RealtimeValueError::InvalidJson));
        }
        for raw in [
            &b"{"[..],
            &br"[]"[..],
            &br"null"[..],
            &br#""not an event""#[..],
        ] {
            assert_eq!(decode_server_event(raw), Err(RealtimeValueError::InvalidJson));
        }
        for raw in [&br"{}"[..], &br#"{"type":null}"#[..], &br#"{"type":42}"#[..]] {
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
