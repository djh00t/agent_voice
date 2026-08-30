# Task 10b.1 report: Realtime opaque identifiers and redacted decode errors

- **Issue:** [#217](https://github.com/djh00t/agent_voice/issues/217)
- **Package:** `task-10b.1`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-10b2-realtime-primitives`
- **Branch:** `codex/agent-voice-pa-10b2-realtime-primitives`
- **Base:** `4ba837e6ed7f2cd4ba431660865d902a2787f9eb` (`origin/main`)
- **Implementation commits:** `c8bdf05`, `4b1eee2`, `55edd2a`, `bba1bab`

## Scope and ownership

This package owns the bounded Realtime value/error foundation:

- `src/realtime/values.rs` defines `MAX_EVENT_BYTES` (`65_536`),
  `MAX_ID_BYTES` (`128`), `MAX_ERROR_MESSAGE_CHARS` (`512`), the closed
  `RealtimeValueError` set, and validated serde `OpaqueId` values.
- `src/realtime/values.rs` contains the focused selector test, including the
  exact identifier and malformed-JSON boundaries.
- `src/realtime/mod.rs` registers the value module and documents the value
  boundary so the issue-mandated selector is available from #215's bootstrap.

No audio or text codecs, event schemas, dispatch, configuration, PA behavior,
I/O, logging, state, provider access, or dependencies were added.

## Contract mapping

`OpaqueId` is nonempty ASCII, at most 128 bytes, and accepts only ASCII
alphanumeric characters plus `-`, `_`, `.`, and `:`. It has no prefix
interpretation, serializes as one JSON string, rejects non-string or invalid
JSON values during deserialization, and never includes an input value in its
error.

`RealtimeValueError` has exactly the issue-defined fifteen variants. Its
`Display` implementation emits fixed short messages and intentionally ignores
the `MissingField` payload, so values, raw JSON, provider messages, and
identifiers cannot be exposed through formatting. All messages remain below
`MAX_ERROR_MESSAGE_CHARS`.

## RED evidence

After adding only the selector-shaped test and module registration, before
implementing the production values, the mandated selector failed as expected:

```text
rtk cargo test --lib realtime::values::tests::opaque_ids_and_redacted_errors -- --exact
test result: FAILED. 0 passed; 1 failed; 481 filtered out
realtime values are not implemented yet
```

Per the updated issue contract, this captured command output is valid RED
evidence; no broken immutable commit is required.

## GREEN evidence

The same selector passes after the minimal implementation:

```text
rtk cargo test --lib realtime::values::tests::opaque_ids_and_redacted_errors -- --exact
cargo test: 1 passed, 481 filtered out (1 suite, 0.00s)
```

The focused test covers accepted punctuation, exactly 128 ASCII bytes,
empty/non-ASCII/space/oversize IDs, JSON string round trips, non-string and
malformed JSON syntax, each exact error display, payload redaction, and the
error-message bound.

## Validation evidence (LOCAL)

| Check | Result |
| --- | --- |
| `rtk cargo test --lib` | PASS — 482 passed (1 suite, 12.61s) |
| `rtk rustfmt --edition 2024 --check src/realtime/values.rs src/realtime/mod.rs` | PASS |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk git diff --check origin/main..HEAD` | PASS |
| `rtk make check` | PASS — Rust tests, docs, lint, and Docusaurus completed with exit 0 |

The first package run of `rtk make check` stopped at `docs-build` because this
fresh worktree had no `website/node_modules` (`docusaurus: command not found`).
`rtk make docs-install` installed dependencies from the checked-in lockfile;
no manifest or lockfile changed. The repair rerun above passed.

The whole-crate `rtk cargo fmt -- --check` reports pre-existing formatting
differences in unrelated `src/pa/fakes/calendar.rs`, `src/pa/fakes/mail.rs`,
and `src/service.rs`; the owned files pass the scoped check above and those
files were not changed.

## Non-claims and handoff

- **CI:** not run or observed in this local package worktree.
- **LIVE:** no provider, credential, network, SIP, audio, or deployment
  behavior was exercised.
- **Delivery:** commits are local only; no push, PR, merge, or approval was
  performed. The issue contract/evidence comment is recorded at
  [#217 comment 5468619710](https://github.com/djh00t/agent_voice/issues/217#issuecomment-5468619710).
- **Follow-on:** #219 may extend `values.rs` with the separately scoped text,
  argument, tool-output, and G.711 mu-law codecs.

## Lifecycle linkage

`Closes #217`

`Refs #97`

## Package status

`status:review` / locally verified within the issue #217 scope.
