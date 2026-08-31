# Task 10b.7 report: Realtime response completion and interruption values

- **Issue:** [#224](https://github.com/djh00t/agent_voice/issues/224)
- **Package:** `task-10b.7`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Worktree:** `/Users/djh/.codex/worktrees/agent_voice-224`
- **Branch:** `codex/issue-224`
- **Base:** `5469cad6` (`origin/main`)

## Contract and readback

The prerequisite delivery for #217 is merged by PR #233 at
`57643df4410b217743e8e582fa844cea8864b7fb`. The prerequisite delivery for
#219 is merged by PR #245 at `a160254804c2d3e78631057af29530b5f96c17dd`.
Both are reachable from the clean `origin/main` base. The readback confirmed
the shared interfaces in `src/realtime/values.rs`: `OpaqueId`,
`RealtimeValueError`, `TranscriptText`, `FunctionArguments`, `ToolOutput`,
`AudioCodec`, and `G711Ulaw`. The response package reuses those values and
does not redefine them.

The original library selector could not discover this pre-registration source
file without violating #225's registration ownership. The binding amendment
at [#224 comment 5476962230](https://github.com/djh00t/agent_voice/issues/224#issuecomment-5476962230)
therefore authorizes the unique integration harness below. It includes the
real source files by path and does not modify `src/realtime/mod.rs`.

## Owned paths and hunks

The final package owns exactly these three paths:

- `src/realtime/server_response_events.rs`: closed response status,
  interruption reason, provider error, response summary, and `response.done`
  values, plus inline focused tests. `ResponseSummary::new` validates
  construction, and its serializer validates direct public values at the
  serialization boundary.
- `tests/realtime_server_response_events_contract.rs`: the amended guarded
  pre-registration harness. It exposes a test-only `realtime::values` alias,
  includes the real values module, and includes the real response source by
  path. It contains no copied production implementation or dispatcher.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md`: this report.

The final changed-path readback after the report commit is:

```text
src/realtime/server_response_events.rs
tests/realtime_server_response_events_contract.rs
.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md
```

No registration, export, dispatcher, transport, provider, configuration,
dependency, or sibling event file was changed.

## RED

The guarded listing and exact selector were run before the owned source file
existed. Both failed nonzero because the harness path target was absent:

```text
rtk cargo test --test realtime_server_response_events_contract -- --list
exit 101
error: couldn't read tests/../src/realtime/server_response_events.rs: No such file or directory (os error 2)
```

```text
rtk cargo test --test realtime_server_response_events_contract server_response_events::tests::response_done_interruptions -- --exact
exit 101
error: couldn't read tests/../src/realtime/server_response_events.rs: No such file or directory (os error 2)
```

After adding the inline test module but before adding production definitions,
the same exact selector failed for the expected missing-symbol reason:

```text
rtk cargo test --test realtime_server_response_events_contract server_response_events::tests::response_done_interruptions -- --exact
exit 101
error[E0432]: unresolved imports ... InterruptionReason, ProviderErrorSummary,
RealtimeServerResponseEvent, ResponseStatus, ResponseStatusDetails, ResponseSummary
```

No zero-test success was counted as RED evidence.

## GREEN

The amended listing was read back with the unfiltered proxy so the matched
test is visible:

```text
rtk proxy cargo test --test realtime_server_response_events_contract -- --list
exit 0
server_response_events::tests::response_done_interruptions: test
values::tests::bounded_text_and_g711_ulaw: test
values::tests::opaque_ids_and_redacted_errors: test
3 tests, 0 benchmarks
```

The mandated focused selector then passed with one matched test:

```text
rtk cargo test --test realtime_server_response_events_contract server_response_events::tests::response_done_interruptions -- --exact
exit 0
cargo test: 1 passed, 2 filtered out (1 suite, 0.00s)
```

The full amended harness also passed:

```text
rtk cargo test --test realtime_server_response_events_contract
exit 0
cargo test: 3 passed (1 suite, 0.00s)
```

The review repair was developed with a genuine RED/GREEN cycle. After the
regression assertions were added but before the construction and serialization
boundary was repaired, the exact selector failed with the expected missing
constructor errors:

```text
rtk cargo test --test realtime_server_response_events_contract server_response_events::tests::response_done_interruptions -- --exact
exit 101
error[E0599]: no function or associated item named `new` found for struct `ResponseSummary`
at tests/realtime_server_response_events_contract.rs:215 and :242
2 errors, 1 warning
```

After `10b2018`, the same selector passed and covered both invalid paths and a
valid cancelled round trip:

```text
rtk cargo test --test realtime_server_response_events_contract server_response_events::tests::response_done_interruptions -- --exact
exit 0
cargo test: 1 passed, 2 filtered out (1 suite, 0.00s)
```

The invalid constructor returns the shared redacted
`InvalidInterruptionReason` classification. Direct public construction cannot
serialize a non-cancelled response with a reason: both `ResponseSummary` and
the enclosing `response.done` event fail at serialization. A cancelled value
constructed with `ResponseSummary::new` serializes and deserializes unchanged.

The focused test covers all five statuses, both cancellation reasons,
cancellation-only reason validation at construction and serialization,
optional status details and provider error fields, required/null IDs and
response/status fields, unknown statuses, unknown reasons, unknown tags/fields,
malformed JSON, exact snake_case wire values, and redaction of rejected
payloads and successful-value debug output.

## Checks

| Command | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 --check src/realtime/server_response_events.rs tests/realtime_server_response_events_contract.rs` | PASS, exit 0 |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS, exit 0; no issues found |
| `rtk git diff --check` | PASS, exit 0 |
| `rtk make docs-install` | PASS, exit 0; installed website dependencies from the checked-in lockfile |
| `rtk make check` | PASS, exit 0 after docs dependencies were installed; Rust tests, Clippy, rustdoc, and Docusaurus completed |

The first `rtk make check` attempt exited 2 at `docs-build` because the fresh
worktree had no `docusaurus` executable (`docusaurus: command not found`). The
rerun after `rtk make docs-install` passed. npm reported 24 existing audit
advisories (7 moderate, 17 high); no remediation was performed.

The required whole-tree formatter command was also run:

```text
rtk cargo fmt --all -- --check
exit 1
```

Its output contains only pre-existing formatting differences in unrelated
`src/pa/fakes/mail.rs` and `src/service.rs`; the scoped formatter command for
the owned production and harness files passed. No unrelated file was changed.

## Scope, security, and residuals

The event boundary emits and accepts exactly one tagged event,
`type: "response.done"`, with required non-null `event_id` and `response`.
`ResponseSummary` is exactly `{id,status,status_details}`; status is closed to
`in_progress`, `completed`, `cancelled`, `failed`, and `incomplete`. Details
are exactly `{reason,error}`; reason is closed to `turn_detected` and
`client_cancelled`, while provider error is exactly `{type,code}` with an
optional code. Unknown tags, fields, statuses, reasons, malformed values,
missing/null required fields, and non-cancelled interruption reasons fail
closed with fixed redacted error classifications.

Successful values preserve provider IDs and optional provider error fields for
later integration. `Debug` implementations for response values redact those
fields. `Display` for all failures comes from the shared redacted
`RealtimeValueError` contract, so raw JSON, IDs, provider messages, reasons,
and status values do not appear in failure text. Decode and validation are
pure and deterministic; no cancellation command, playback, session update,
provider call, queue, persistence, filesystem, environment, network, or
other state mutation occurs. Lifecycle and deduplication remain outside this
package.

CI, provider/live behavior, credentials, deployment, network, and
authenticated UAT were not run or observed locally.

## Delivery readback

Package commits are one-file atomic commits. The original pre-rebase delivery
readback was:

- `a9be996` — harness only: `tests/realtime_server_response_events_contract.rs`
- `90cc705` — implementation and inline test only:
  `src/realtime/server_response_events.rs`
- `6687be0` — report only:
  `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md`
- `eae96d8` — final report-only readback:
  `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md`

After rebasing onto the current `origin/main`, the equivalent delivery and
repair commits are:

- `b18b10a` — harness only: `tests/realtime_server_response_events_contract.rs`
- `7a6cbe7` — implementation and inline test only:
  `src/realtime/server_response_events.rs`
- `001f59c` — report only:
  `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md`
- `661c3ee` — report-only finalization:
  `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md`
- `10b2018` — repair implementation and regression test only:
  `src/realtime/server_response_events.rs`
- `04b7533` — report-only rebased readback:
  `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b7-report.md`

This report update is the next report-only repair commit and records the
review evidence above; all listed commits preserve the exact-one-file rule.

The delivering PR footer is exactly:

```text
Closes #224
Refs #97
Refs #219
Refs #217
```

Parent tracker #97 remains open. No merge or approval is performed by this
package.
