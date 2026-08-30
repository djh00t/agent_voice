# Task 10b.2 report: Realtime text and G.711 mu-law value codecs

- **Issue:** [#219](https://github.com/djh00t/agent_voice/issues/219)
- **Package:** `task-10b.2`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-10b3-realtime-codecs`
- **Branch:** `codex/agent-voice-pa-10b3-realtime-codecs`
- **Base:** `57643df4410b217743e8e582fa844cea8864b7fb` (`origin/main`, merged PR #233)
- **Implementation commit:** `beb17d4`

## Scope and ownership

This package changes only the two issue-owned paths:

- `src/realtime/values.rs` adds `TranscriptText`, `FunctionArguments`,
  `ToolOutput`, `AudioCodec`, and `G711Ulaw`, with bounded constructors,
  redacted failures, serde wire forms, and the focused test.
- This report records the contract and actual command evidence.

No event enums, parser routing, websocket or transport code, dispatch,
playback, configuration, dependencies, registrations, public exports, legacy
bridge, or persistent data were changed.

## Contract mapping

- `TranscriptText` preserves its source string exactly and accepts at most
  4096 Unicode scalar values; longer text returns `TranscriptTooLong`.
- `FunctionArguments` preserves opaque UTF-8 text up to 16384 bytes. The
  crate-local `from_delta` constructor checks only the byte bound, while
  `from_completed` checks the bound before accepting only a valid JSON object;
  arrays, scalars, null, malformed JSON, and oversize input return the issue
  errors without trimming or reserialization.
- `ToolOutput` preserves arbitrary UTF-8 text up to 16384 bytes and has no
  parsing or execution behavior; oversize input returns `ToolOutputTooLong`.
- `AudioCodec` is closed and accepts/serializes only `g711_ulaw` as
  `G711Ulaw`; unsupported names return `UnsupportedAudioFormat`.
- `G711Ulaw` preserves nonempty opaque bytes up to 16384 bytes and serializes
  as standard padded RFC 4648 base64. Deserialization checks the 21848-character
  encoded bound before decoding, requires ASCII canonical standard base64, and
  verifies exact re-encoding. Empty bytes return `EmptyAudio`; malformed or
  noncanonical input returns `InvalidBase64`; oversize encoded or decoded input
  returns `AudioTooLarge`.
- Sensitive wrapper `Debug` output and serde type errors are redacted; no
  rejected payload is included in value errors. Failed construction or decode
  produces no partial value or I/O, queue, sink, playback, tool, or state
  mutation.

## RED evidence

After adding the focused test and before adding the production symbols, the
mandated selector failed because the requested types were absent:

```text
rtk cargo test --lib realtime::values::tests::bounded_text_and_g711_ulaw -- --exact
cargo test: 26 errors, 0 warnings (1 crates)
error[E0425]: cannot find type `TranscriptText` in this scope
error[E0425]: cannot find type `FunctionArguments` in this scope
error[E0425]: cannot find type `ToolOutput` in this scope
error[E0425]: cannot find type `AudioCodec` in this scope
error[E0425]: cannot find type `G711Ulaw` in this scope
... +6 more issues
exit 101
```

## GREEN and validation evidence

| Check | Result |
| --- | --- |
| `rtk cargo test --lib realtime::values::tests::bounded_text_and_g711_ulaw -- --exact` | PASS — 1 passed, 498 filtered out |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk rustfmt --edition 2024 --check src/realtime/values.rs` | PASS |
| `rtk git diff --check` | PASS |
| `rtk make check` | PASS — Rust tests (499), clippy, Rust docs, and Docusaurus completed with exit 0 |

The first `rtk make check` attempt stopped at `docs-build` because this fresh
worktree had no `website/node_modules` (`docusaurus: command not found`).
`rtk make docs-install` then installed from the checked-in lockfile; it did not
change package manifests or lockfiles. The rerun passed. npm reported 24
existing audit advisories (7 moderate, 17 high); no audit remediation was
performed.

The mandated whole-tree `rtk cargo fmt --all -- --check` reports pre-existing
formatting differences in unrelated `src/pa/fakes/calendar.rs`,
`src/pa/fakes/mail.rs`, and `src/service.rs`; the owned file passes the scoped
formatter check above and those unrelated files were not changed.

The clean baseline before this package was `rtk cargo test --lib` — 498 passed.

## Non-claims and delivery

- **CI:** not run or observed in this local package worktree.
- **LIVE:** no provider, credential, network, SIP, transport, queue, sink,
  playback, tool, deployment, or authenticated UAT behavior was exercised.
- **Persistence:** no persistent data or migration exists; rollback is reverting
  `beb17d4` and this report commit.
- **Delivery:** the source commit is ready to push and publish for review; no
  merge or approval is performed by this package.

## Lifecycle linkage

`Closes #219`

`Refs #97, #217`

## Package status

`status:review` after verified PR publication; parent tracker #97 remains open.
