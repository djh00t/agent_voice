# Task 10b.8 report: closed Realtime server dispatcher

- **Issue:** [#225](https://github.com/djh00t/agent_voice/issues/225)
- **Package:** `task-10b.8`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Worktree:** `/tmp/agent-voice-issue-225`
- **Branch:** `codex/agent-voice-issue-225`
- **Base:** `a20a28be3be37c84cbe5046415497b7053dd8906` (`origin/main`)

## Contract and readback

The fresh `origin/main` readback is
`a20a28be3be37c84cbe5046415497b7053dd8906`, which contains the merged PR #304
delivery for #222 and subsequent PR #305, #306, #307, and #309 merges. The
required prerequisite
merge commits were each checked with `git merge-base --is-ancestor` and all
returned exit 0:

| Issue | Delivery | Merge commit |
| --- | --- | --- |
| #215 | PR #228 | `4ba837e6ed7f2cd4ba431660865d902a2787f9eb` |
| #217 | PR #233 | `57643df4410b217743e8e582fa844cea8864b7fb` |
| #219 | PR #245 | `a160254804c2d3e78631057af29530b5f96c17dd` |
| #220 | PR #296 | `5799e89978e335783f8c05b14b2b04cf9292251c` |
| #221 | PR #303 | `5469cad6862f69264cb55b159c4443038fa84864` |
| #222 | PR #304 | `eb9c791b46b057b41c0dd69284014fcfab48826f` |
| #223 | PR #301 | `b1b83562b3f2f92b58445448a76ab770a977597f` |
| #224 | PR #302 | `f8319aceec157d974b52dca73e192b34653f25d1` |

Before editing, the exact source paths and public names were read from
`origin/main`: `src/realtime/values.rs`, `src/realtime/client_events.rs`,
`src/realtime/server_session_events.rs`, `src/realtime/server_audio_events.rs`,
`src/realtime/server_function_events.rs`,
`src/realtime/server_response_events.rs`, and `src/realtime/mod.rs`.
`src/lib.rs` retains its existing `pub mod realtime;` line and was not edited.
No dependency or lockfile was changed.

The implementation was rebased onto this current `origin/main` before
publication. PR #305 changes are outside the owned Realtime paths; all eight
prerequisite SHAs remain ancestors of the rebased base.

## Owned paths and hunks

The final package owns exactly these three paths:

- `src/realtime/mod.rs`: the exact public module registration and six-name
  `pub use` block from the binding amendment.
- `src/realtime/events.rs`: the closed four-variant wrapper, duplicate-aware
  pure byte decoder, child-boundary parsers, serializer, redacted formatting,
  and inline `realtime::events::tests::closed_server_dispatch_matrix` test.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b8-report.md`: this report.

The final changed-path readback after the report commit is:

```text
src/realtime/events.rs
src/realtime/mod.rs
.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b8-report.md
```

The exact root exports are `decode_server_event`, `RealtimeServerEvent`,
`RealtimeServerAudioEvent`, `RealtimeServerFunctionEvent`,
`RealtimeServerResponseEvent`, and `RealtimeServerSessionEvent`. No client
event, payload, colliding `FunctionCallOutputType`, value primitive, serde
helper, or wildcard is root-re-exported.

The accepted server matrix is exactly:

| Wire tag | Wrapper and child variant |
| --- | --- |
| `session.created` | `Session(SessionCreated)` |
| `session.updated` | `Session(SessionUpdated)` |
| `error` | `Session(Error)` |
| `input_audio_buffer.committed` | `Session(InputAudioBufferCommitted)` |
| `input_audio_buffer.cleared` | `Session(InputAudioBufferCleared)` |
| `input_audio_buffer.speech_started` | `Session(InputAudioBufferSpeechStarted)` |
| `input_audio_buffer.speech_stopped` | `Session(InputAudioBufferSpeechStopped)` |
| `conversation.item.input_audio_transcription.delta` | `Session(ConversationItemInputAudioTranscriptionDelta)` |
| `conversation.item.input_audio_transcription.completed` | `Session(ConversationItemInputAudioTranscriptionCompleted)` |
| `response.output_audio.delta` | `Audio(OutputAudioDelta)` |
| `response.output_audio.done` | `Audio(OutputAudioDone)` |
| `response.output_audio_transcript.delta` | `Audio(OutputAudioTranscriptDelta)` |
| `response.output_audio_transcript.done` | `Audio(OutputAudioTranscriptDone)` |
| `response.function_call_arguments.delta` | `Function(FunctionCallArgumentsDelta)` |
| `response.function_call_arguments.done` | `Function(FunctionCallArgumentsDone)` |
| `conversation.item.created` | `Function(ConversationItemCreated)` |
| `response.done` | `Response(ResponseDone)` |
| #220 smoke: `response.cancel` | `RealtimeClientEvent::ResponseCancel`; client-only round trip, dispatcher rejects it |

## RED

After the test-only commit and before production definitions existed, the exact
focused selector failed nonzero for the expected missing-symbol reason:

```text
rtk cargo test --lib realtime::events::tests::closed_server_dispatch_matrix -- --exact
exit 101
error[E0432]: unresolved imports `super::RealtimeServerEvent`, `super::decode_server_event`
error[E0432]: unresolved imports `events::decode_server_event`, `events::RealtimeServerEvent`
```

This was not a zero-test success; the selector could not compile because the
dispatcher contract was intentionally absent.

## GREEN

The required nonzero-match list guards all exited 0 and each exposed exactly
one matching test through the unfiltered `rtk run` readback:

| Selector | `rtk cargo test ... -- --list` | Matching test readback |
| --- | --- | --- |
| `realtime::events::tests::closed_server_dispatch_matrix` | exit 0 | 1 test |
| `realtime::client_events::tests::closed_client_events` | exit 0 | 1 test |
| `realtime::server_session_events::tests::session_and_caller_events` | exit 0 | 1 test |
| `realtime::server_audio_events::tests::output_audio_events` | exit 0 | 1 test |
| `realtime::server_function_events::tests::function_call_events` | exit 0 | 1 test |
| `realtime::server_response_events::tests::response_done_interruptions` | exit 0 | 1 test |

Each exact selector then exited 0 with `1 passed, 571 filtered out` on the
rebased base; no
filtered-only or zero-match result was counted. The focused dispatcher matrix
also verifies every accepted tag above, exact wire re-encoding, client-only
rejection, obsolete `response.audio.*` aliases, unknown tags/fields, malformed
and non-object input, duplicate members at nested levels, type routing,
required/null fields, opaque ID bounds, transcript/argument/tool-output/audio
bounds, completed-argument object validation, response status/reason errors,
fixed redacted debug/display output, and child-compatible MissingField
precedence when recognized aggregate events contain unknown members.

## Checks

| Command | Result |
| --- | --- |
| `rtk cargo test --lib realtime::events::tests::closed_server_dispatch_matrix -- --exact` | PASS, exit 0; 1 passed |
| Six required child exact selectors | PASS, exit 0 each; 1 passed each |
| `rtk cargo test --lib` | PASS, exit 0; 572 passed on rebased base |
| `rtk rustfmt --edition 2024 --check src/realtime/events.rs` | PASS, exit 0 |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS, exit 0; no issues found |
| `rtk git diff --check` | PASS, exit 0 |
| `rtk make docs-install` | PASS, exit 0; used the checked-in website lockfile |
| `rtk make check` | PASS, exit 0 after docs dependencies were installed; Rust tests, lint, rustdoc, and Docusaurus completed |

The first `rtk make check` attempt exited 2 at `docs-build` because the fresh
worktree had no `docusaurus` executable. The rerun after `rtk make docs-install`
passed. npm reported 24 existing audit advisories (7 moderate, 17 high); no
remediation or manifest/lockfile change was performed.

The mandated whole-tree formatter command was also run:

```text
rtk cargo fmt --all -- --check
exit 1
```

Its output includes the binding-mandated exact ordering/export spelling in
`src/realtime/mod.rs`, plus pre-existing formatting differences in
`src/pa/fakes/mail.rs`, `src/service.rs`, and
`src/realtime/server_audio_events.rs`. The scoped formatter for the owned
implementation passed, and no unrelated file was changed.

## Scope, security, and residuals

`decode_server_event` checks the raw byte bound before parsing, rejects
malformed/non-object/duplicate/unknown-field input with fixed redacted errors,
and routes missing/null/non-string/unlisted `type` to `UnknownEventType`.
Recognized events are parsed into the existing child types without aliases or
catchalls. Aggregate dispatch consumes required fields before rejecting
unknown members, preserving child decoder `MissingField` precedence; session
dispatch retains its established session-child validation. IDs,
indexes/timestamps, UTF-8 transcript text, function argument
fragments/completed objects, tool output, canonical padded G.711 mu-law base64,
response statuses, and interruption reasons retain their existing bounds and
typed failures. No raw JSON, ID, index, audio, transcript, argument, tool
output, model, or provider message is emitted by dispatcher formatting or
errors.

The implementation is pure, deterministic, replay-inert, and has no queue,
sink, playback, provider, transport, websocket, SIP/RTP, session, PA, tool,
filesystem, environment, network, persistence, OAuth, credential, deployment,
or publication action. It does not deduplicate, order, retry, cancel,
acknowledge, execute, forward, or mutate state. The #220 client smoke remains
serde-only and is never dispatched as a server event.

LOCAL focused tests/checks and STATIC scope/schema/security review were run.
CI, peer review, provider/live behavior, credentials, deployment, network, and
authenticated UAT were not run or observed locally.

## Delivery readback

The four commits are atomic, each touching exactly one file, with the required
subjects:

1. Module registration: `48cafe0` —
   `test(realtime): register final event modules` —
   `src/realtime/mod.rs` only.
2. Test: `6454788` —
   `test(realtime): define closed server dispatch matrix` —
   `src/realtime/events.rs` only, focused test before implementation; includes
   wrapper-specific non-string identifier, audio, function-item type, and
   child-compatible missing-field precedence regressions.
3. Implementation: `7ea7095` —
   `feat(realtime): implement closed server dispatcher` —
   `src/realtime/events.rs` only; preserves the session child `InvalidJson`
   classification while matching aggregate child `InvalidOpaqueId` and
   `InvalidBase64` errors, function-item `UnknownEventType` errors, and
   required-field-before-unknown-member precedence.
4. Report: explicit `HEAD` self-reference —
   `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b8-report.md` only. The
   report deliberately does not name its own commit SHA, because amending this
   report would otherwise make that hash stale.

Rollback is `git revert` of those four commits in reverse order; there is no
migration, persistent schema, provider state, or external side effect.

The delivering PR footer is exactly:

```text
Closes #225
Refs #97
Refs #217
Refs #219
Refs #220
Refs #221
Refs #222
Refs #223
Refs #224
```

Parent tracker #97 remains open. No merge or approval is performed by this
package.
