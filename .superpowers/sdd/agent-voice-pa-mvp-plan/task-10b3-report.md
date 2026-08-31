# Task 10b.3 report: Realtime outbound client event values

## Contract and readback

- Issue: #220 (`[Task 10b.3] Realtime outbound client event values`).
- Evidence timestamp: 2026-08-31T19:38:55+1000 (Australia/Sydney).
- Base: `9daaefec70666f1bd4e35396bd4385136ab45992` (`origin/main`).
- Merged prerequisite #217: PR #233, merge commit
  `57643df4410b217743e8e582fa844cea8864b7fb`.
- Merged prerequisite #219: PR #245, merge commit
  `a160254804c2d3e78631057af29530b5f96c17dd`.
- Readback files: `src/realtime/values.rs` supplied `OpaqueId`,
  `AudioCodec::G711Ulaw`, `G711Ulaw`, `ToolOutput`, and redacted
  `RealtimeValueError`; `src/realtime/mod.rs` supplied the existing
  `pub mod values;` registration and inert test module.

## Owned paths and hunks

The only changed paths are:

- `src/realtime/client_events.rs` in its entirety: the five requested public
  value types, closed serde implementations, redacted debug output, and the
  single inline `closed_client_events` test.
- `src/realtime/mod.rs`: exactly the test-only declaration
  `#[cfg(test)] mod client_events;` immediately after `pub mod values;`.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b3-report.md`: this report.

The final changed-path readback is:

```text
rtk git diff --name-only 9daaefec70666f1bd4e35396bd4385136ab45992...HEAD
 .superpowers/sdd/agent-voice-pa-mvp-plan/task-10b3-report.md
 src/realtime/client_events.rs
 src/realtime/mod.rs
```

No other source, registration, export, dispatcher, documentation,
configuration, dependency, lockfile, or generated path is owned or changed.

## RED

Exact command:

```text
rtk cargo test --lib realtime::client_events::tests::closed_client_events -- --exact
```

Exit code: `101`.

Sanitized actual failure excerpt:

```text
cargo test: 1 errors, 0 warnings (1 crates)
error[E0432]: unresolved imports
`super::FunctionCallOutputItem`, `super::FunctionCallOutputType`,
`super::RealtimeClientEvent`, `super::SessionUpdatePayload`,
`super::TurnDetection`
```

The selector failed because the registered test referenced the absent
production value contract; no production definitions existed at RED time.

## GREEN

Exact selector:

```text
rtk cargo test --lib realtime::client_events::tests::closed_client_events -- --exact
```

Exit code: `0`; matched `1`, passed `1`, failed `0`, filtered out `530`.

Actual sanitized result:

```text
cargo test: 1 passed, 530 filtered out (1 suite, 0.00s)
```

The focused test covers all six event tags, exact fields, present/absent/null
optional IDs, omitted optional fields, canonical G.711 mu-law base64, exact
model and tool output preservation, both accepted VAD values, unsupported
codec/VAD/item/type/tag values, unknown fields, malformed nested values,
missing/null required fields, and redacted error/debug text.

## Checks

| Command | Exit code | Actual result |
| --- | ---: | --- |
| `rtk cargo fmt --all -- --check` | `1` | Pre-existing differences only in unrelated `src/pa/fakes/mail.rs` and `src/service.rs`; the owned client file was formatted with `rtk rustfmt --edition 2024 src/realtime/client_events.rs`. |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | `0` | `cargo clippy: No issues found`. |
| `rtk git diff --check` | `0` | No whitespace errors. |
| `rtk make check` | `0` | Rust tests (531), Clippy, Rust docs, and Docusaurus completed successfully. |

The first `rtk make check` attempt exited `2` only because the fresh worktree
had no `website/node_modules` (`docusaurus: command not found`). `rtk npm ci`
in `website/` then installed the checked-in lockfile (1276 packages added,
1277 audited), without changing package manifests or lockfiles; the exact
`rtk make check` rerun passed. npm reported 24 existing audit advisories
(7 moderate, 17 high); no audit remediation was performed.

## Scope, security, and residuals

The closed wire tag matrix is exactly:

| Rust variant | Wire tag | Additional fields |
| --- | --- | --- |
| `SessionUpdate` | `session.update` | optional `event_id`, required `session` |
| `InputAudioBufferAppend` | `input_audio_buffer.append` | optional `event_id`, required `audio` |
| `InputAudioBufferCommit` | `input_audio_buffer.commit` | optional `event_id` |
| `InputAudioBufferClear` | `input_audio_buffer.clear` | optional `event_id` |
| `ResponseCancel` | `response.cancel` | optional `event_id` |
| `ConversationItemCreate` | `conversation.item.create` | optional `event_id`, required `item` |

`SessionUpdatePayload` accepts only the required model and two shared audio
codecs plus optional `TurnDetection`; VAD accepts only `server_vad` and
`semantic_vad`. `FunctionCallOutputItem` accepts only the optional ID,
`function_call_output` item type, required call ID, and opaque `ToolOutput`.
Unknown tags/fields and malformed shapes fail closed as `UnknownEventType` or
`InvalidJson`; missing/null required fields return redacted `MissingField`;
unsupported codecs return `UnsupportedAudioFormat`; shared ID/audio/tool
validation remains bounded and redacted. Optional IDs and VAD are omitted
when absent and accepted when absent or JSON `null`.

Serialization/deserialization is pure and deterministic. Errors and custom
debug output never include rejected JSON, tags, field values, IDs, codec/VAD
strings, audio/base64, model text, tool output, or provider text. No socket,
queue, sink, playback, provider, filesystem, environment, PA tool, or state
mutation is present; invalid decoding produces no partial value. There is no
event ordering, ID generation, decoding/transcoding, execution,
transport/websocket, server event, dispatcher, registration/export,
configuration, dependency, persistent schema, migration, live-provider,
credential, deployment, or authenticated-UAT work in this package.

## Delivery readback

- Source commit: `53e5161` (`feat(realtime): add closed client event values`).
- Registration commit: `237d22b` (`test(realtime): register client event harness`).
- Warning-fix commit: `efdb0ee` (`fix(realtime): keep client value API warning-free`).
- The report is committed separately as the final one-file delivery commit;
  final `rtk git log --oneline -3` and `rtk git diff --name-only
  9daaefec70666f1bd4e35396bd4385136ab45992...HEAD` are the delivery readback.
- CI, provider/live behavior, credentials, deployment, and authenticated UAT
  are not claimed; only the LOCAL and STATIC evidence above is available.

The delivering PR footer is exactly:

```text
Closes #220
Refs #97
Refs #217
Refs #219
```
