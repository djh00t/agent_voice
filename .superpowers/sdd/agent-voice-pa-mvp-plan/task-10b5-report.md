# Task 10b.5 report: Realtime output-audio server event values

- **Issue:** [#222](https://github.com/djh00t/agent_voice/issues/222)
- **Package:** `task-10b.5`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-issue-222`
- **Branch:** `codex/agent-voice-issue-222`
- **Base:** `1fdfd9fd3d09e95aafd46823fd82bfa8c44a7ac9` (`origin/main`)
- **Implementation commit:** `8b9dee9f7989d17cfe3deddd06b7710f8a37cbb8`
- **Harness commit:** `9bc67efe8227d8b4fb22a15c9e9fcd0b300332f8`

## Contract and readback

The dependency gate was rechecked before implementation. #217 is closed by
merged PR #233 (`57643df4410b217743e8e582fa844cea8864b7fb`), and #219 is
closed by merged PR #245 (`a160254804c2d3e78631057af29530b5f96c17dd`). The
reachable `origin/main` value boundary was read from `src/realtime/values.rs`:
`OpaqueId`, `G711Ulaw`, `TranscriptText`, and `RealtimeValueError` are shared
without redefinition.

The binding pre-registration harness amendment is recorded at
[#222 comment 5476959919](https://github.com/djh00t/agent_voice/issues/222#issuecomment-5476959919).
It permits one unique integration harness while #225 retains production
registration and final library-selector ownership.

## Owned paths and hunks

This package changes exactly these three paths:

- `src/realtime/server_audio_events.rs`: the closed four-variant event value,
  custom serde boundary, inline focused test, and redacted debug behavior.
- `tests/realtime_server_audio_events_contract.rs`: the test-only root
  `values` include, `realtime::values` alias, and real source-file include.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b5-report.md`: this report.

The harness has no copied production implementation, dispatcher, export,
network code, or sibling event module. `src/realtime/mod.rs` was not changed.
The final `git diff --name-only origin/main...HEAD` readback is required to
equal the three paths above.

## RED

After writing the focused test and harness, before defining the production
event type, both mandated guarded commands failed nonzero as required:

```text
rtk cargo test --test realtime_server_audio_events_contract -- --list
exit 101
error[E0432]: unresolved import `super::RealtimeServerAudioEvent`
no `RealtimeServerAudioEvent` in `server_audio_events`

rtk cargo test --test realtime_server_audio_events_contract server_audio_events::tests::output_audio_events -- --exact
exit 101
error[E0432]: unresolved import `super::RealtimeServerAudioEvent`
no `RealtimeServerAudioEvent` in `server_audio_events`
```

This was a genuine absent-production-symbol failure, not a zero-test cargo
success or fabricated output.

## GREEN

The same guarded harness passes after the minimal implementation:

```text
rtk cargo test --test realtime_server_audio_events_contract -- --list
exit 0
server_audio_events::tests::output_audio_events: test
values::tests::bounded_text_and_g711_ulaw: test
values::tests::opaque_ids_and_redacted_errors: test
3 tests, 0 benchmarks

rtk cargo test --test realtime_server_audio_events_contract server_audio_events::tests::output_audio_events -- --exact
exit 0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out

rtk cargo test --test realtime_server_audio_events_contract
exit 0
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The focused test covers exact serialization and round trips for all four
closed tags, canonical padded G.711 mu-law audio, lossless transcript text,
required IDs and indexes, missing/null fields, malformed/noncanonical/
alternate/oversized audio, transcript bounds, unknown tags and fields,
obsolete `response.audio.*` aliases, and redacted failures/debug output.

## Checks

| Check | Result |
| --- | --- |
| `rtk run rustfmt --edition 2024 --check src/realtime/server_audio_events.rs tests/realtime_server_audio_events_contract.rs` | PASS |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk git diff --check origin/main..HEAD` | PASS |
| `rtk make check` | PASS — 537 Rust tests, 3 doc-tests, Clippy, rustdoc, and Docusaurus |

The first `rtk make check` attempt found the fresh-worktree environment had no
Docusaurus executable. `rtk make docs-install` installed from the checked-in
website lockfile; no manifest or lockfile changed. The post-install rerun
passed. npm reported 24 existing audit advisories (7 moderate, 17 high); no
remediation was performed.

The required whole-tree `rtk cargo fmt --all -- --check` was also run. It exits
1 only for pre-existing formatting differences in unrelated
`src/pa/fakes/mail.rs` and `src/service.rs`; both owned paths pass the scoped
formatter check and those unrelated files were not changed.

## Scope, security, and residuals

The wire set is closed to exactly `response.output_audio.delta`,
`response.output_audio.done`, `response.output_audio_transcript.delta`, and
`response.output_audio_transcript.done`. Every event ID and `u32` index is
required and non-null. Delta and done payloads have no extra or interchangeable
fields. Shared values enforce their existing bounds and canonical forms.

All decoding and encoding are deterministic, replay-inert data operations.
Unknown tags/fields and invalid shared values fail before a value is returned.
Display and debug output remains fixed/redacted; no raw JSON, IDs, audio,
transcript, or provider payload is emitted. The package performs no queue,
sink, playback, provider, filesystem, network, environment, persistence, or
state action. Production registration, dispatch, transport, and final matrix
proof remain owned by #225.

CI, provider/live behavior, credentials, deployment, and authenticated UAT
were not run or observed.

## Delivery readback

The branch contains two implementation commits plus this one-file report
commit. Each commit touches exactly one path and uses the required multiline
Conventional Commit format. The final changed-path readback must contain only
the three owned paths listed above. No dependency, `src/realtime/mod.rs`,
dispatcher, export, sibling event, or parent tracker file changed.

The delivering PR footer is exactly:

```text
Closes #222
Refs #97
Refs #217
Refs #219
```

## Lifecycle linkage

`Closes #222`

`Refs #97`

`Refs #217`

`Refs #219`

## Package status

`status:review` after local verification and PR publication; parent tracker
#97 remains open.
