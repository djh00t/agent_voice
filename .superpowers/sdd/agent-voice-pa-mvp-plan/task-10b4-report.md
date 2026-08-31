# Task 10b.4 report: Realtime session and caller-transcript server events

## Contract and readback

- **Issue:** [#221](https://github.com/djh00t/agent_voice/issues/221)
- **Evidence date:** 2026-08-31 20:38 AEST
- **Branch:** `codex/agent-voice-pa-10b4-realtime-events`
- **Base:** `b1b83562b3f2f92b58445448a76ab770a977597f` (`origin/main`)
- **Merged prerequisites:** PR #233 for #217 merged at
  `57643df4410b217743e8e582fa844cea8864b7fb`; PR #245 for #219 merged at
  `a160254804c2d3e78631057af29530b5f96c17dd`; transitive #215 is closed.
- **Readback:** `src/realtime/values.rs` supplies `OpaqueId`,
  `TranscriptText`, and redacted `RealtimeValueError`; `src/realtime/mod.rs`
  remains unchanged and has no production event registration.
- The binding pre-registration harness amendment is recorded in issue comment
  [#5476958617](https://github.com/djh00t/agent_voice/issues/221#issuecomment-5476958617).

## Owned paths and hunks

The package owns exactly these three paths:

- `src/realtime/server_session_events.rs`: closed session, provider-error,
  input-buffer, speech-marker, and caller-transcription values; strict
  deserialization; redacted debug output; and the inline focused test.
- `tests/realtime_server_session_events_contract.rs`: unique pre-registration
  integration harness that includes the real values and event files by path.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b4-report.md`: this report.

The harness exposes the test-only `realtime::values` alias and does not copy
production code, edit `src/realtime/mod.rs`, register production modules, or
include sibling event modules. Before the report commit, the changed-path
readback was:

```text
src/realtime/server_session_events.rs
tests/realtime_server_session_events_contract.rs
```

The final `origin/main...HEAD` readback must contain only the two paths above
plus this report.

## RED

The test and harness commits were made before adding production definitions.
Both guarded commands failed nonzero because the requested symbols were absent:

```text
rtk cargo test --test realtime_server_session_events_contract -- --list
exit 101
cargo test: 1 errors, 1 warnings (238 crates)
error[E0432]: unresolved imports `super::ProviderError`,
`super::RealtimeServerSessionEvent`, `super::SessionInfo`
```

```text
rtk cargo test --test realtime_server_session_events_contract server_session_events::tests::session_and_caller_events -- --exact
exit 101
cargo test: 1 errors, 0 warnings (1 crates)
error[E0432]: unresolved imports `super::ProviderError`,
`super::RealtimeServerSessionEvent`, `super::SessionInfo`
```

These are genuine pre-implementation failures, not a zero-test success.

## GREEN

After implementation, the guarded raw listing was:

```text
rtk proxy cargo test --test realtime_server_session_events_contract -- --list
server_session_events::tests::session_and_caller_events: test
values::tests::bounded_text_and_g711_ulaw: test
values::tests::opaque_ids_and_redacted_errors: test
3 tests, 0 benchmarks
```

The required focused selector therefore matches exactly one event test and
passed:

```text
rtk cargo test --test realtime_server_session_events_contract server_session_events::tests::session_and_caller_events -- --exact
exit 0
cargo test: 1 passed, 2 filtered out (1 suite, 0.00s)
```

The complete harness also passed:

```text
rtk cargo test --test realtime_server_session_events_contract
exit 0
cargo test: 3 passed (1 suite, 0.00s)
```

The focused test covers all nine tags, exact transcript preservation including
`Apt 4B, call 2`, optional provider error fields, required/null fields, unknown
tags and fields, malformed nested values, invalid opaque IDs, transcript bounds,
and redaction of rejected values and debug output.

## Checks

- `rtk rustfmt --edition 2024 --check src/realtime/server_session_events.rs tests/realtime_server_session_events_contract.rs` — **PASS**.
- `rtk cargo clippy --all-targets --all-features -- -D warnings` — **PASS** after removing one `map_identity` lint; no issues found.
- `rtk git diff --check` — **PASS**.
- `rtk cargo fmt --all -- --check` — **exit 1**, pre-existing differences only in unrelated `src/pa/fakes/mail.rs` and `src/service.rs`; the owned files pass the scoped formatter check.
- First `rtk make check` — **exit 2** at `docs-build` because the fresh worktree had no Docusaurus executable (`docusaurus: command not found`); Rust tests and Rust docs had already completed.
- `rtk make docs-install` — **exit 0**, installed from the checked-in website lockfile; npm reported 24 existing audit advisories (7 moderate, 17 high). No manifests or lockfiles changed.
- Fresh `rtk make check` after docs installation — **exit 0**, Rust test/doc, Clippy, and Docusaurus stages completed.

## Scope, security, and residuals

The event enum is closed to exactly these wire tags:

`session.created`, `session.updated`, `error`,
`input_audio_buffer.committed`, `input_audio_buffer.cleared`,
`input_audio_buffer.speech_started`, `input_audio_buffer.speech_stopped`,
`conversation.item.input_audio_transcription.delta`, and
`conversation.item.input_audio_transcription.completed`.

All event and nested objects reject unknown fields. Required IDs, timestamps,
indexes, sessions, errors, and transcript fields reject absence/null. Optional
provider error `code`, `param`, and nested `event_id` accept absence/null.
Shared `OpaqueId` and `TranscriptText` values are reused without redefinition.
Decode/encode operations are deterministic and side-effect free; no transport,
queue, playback, provider, PA tool, filesystem, network, environment,
persistence, or state mutation is present. Debug/error output does not expose
IDs, transcript text, or provider details. No CI, provider/live, credentials,
deployment, or authenticated-UAT evidence is claimed here.

## Delivery readback

Implementation commits are atomic and one-file scoped:

- `3f23631` — focused test in `src/realtime/server_session_events.rs`.
- `0fb5bd9` — integration harness in
  `tests/realtime_server_session_events_contract.rs`.
- `2685afb` — event values and implementation in
  `src/realtime/server_session_events.rs`.
- The report updates are separate report-only commits and do not alter the
  implementation or integration harness.

The delivering PR footer is exactly:

```text
Closes #221
Refs #97
Refs #217
Refs #219
```

No `Closes #97` is used. The parent tracker remains open pending all sibling
packages and final dispatcher/registration acceptance.
