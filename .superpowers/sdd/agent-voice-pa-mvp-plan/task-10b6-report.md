# Task 10b.6 report: Realtime function-call server event values

## Contract and readback

- **Issue:** [#223](https://github.com/djh00t/agent_voice/issues/223)
- **Evidence timestamp:** 2026-08-31 20:33 AEST (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-issue-223`
- **Branch:** `codex/agent-voice-issue-223`
- **Base:** `bc2edf55eae758c6cfe0475f25ad76a331276a6b` (`origin/main`)
- **Merged #217 prerequisite:** PR #233, merge commit `57643df4410b217743e8e582fa844cea8864b7fb`.
- **Merged #219 prerequisite:** PR #245, merge commit `a160254804c2d3e78631057af29530b5f96c17dd`.

The readback confirmed `src/realtime/values.rs` supplies the shared
`OpaqueId`, `RealtimeValueError`, `FunctionArguments`, and `ToolOutput`
contracts. `FunctionArguments::from_delta` is the fragment path and
`FunctionArguments::from_completed` is the bounded object-only completion
path. The shared errors have fixed redacted display text.

## Owned paths and hunks

The final package owns exactly these three paths:

- `src/realtime/server_function_events.rs`: closed delta, done, and
  `conversation.item.created` values, their inline focused test, and custom
  serde boundaries.
- `tests/realtime_server_function_events_contract.rs`: the guarded
  pre-registration harness that includes the real value and event source
  files, with a test-only `realtime::values` alias. It contains no copied
  production implementation, registration, dispatcher, or sibling event.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-10b6-report.md`: this report.

`src/realtime/mod.rs` was not changed. The final readback command is:

```text
rtk git diff --name-only bc2edf55eae758c6cfe0475f25ad76a331276a6b...HEAD
```

After this report commit it must list only the three paths above.

## RED

The amended guarded harness was used because the production module is
intentionally unregistered until #225. Before the production definitions:

- `rtk cargo test --test realtime_server_function_events_contract -- --list`
  exited **101** because the test target did not yet exist (`error: no test
  target named realtime_server_function_events_contract`).
- After adding the harness and focused test, but before implementation,
  `rtk cargo test --test realtime_server_function_events_contract
  server_function_events::tests::function_call_events -- --exact` exited
  **101** with unresolved imports for `FunctionCallOutputAckItem`,
  `FunctionCallOutputType`, and `RealtimeServerFunctionEvent` in the real
  source file.

These are genuine missing-contract failures; no output was fabricated.

## GREEN

The guarded list command completed with exit **0**:

```text
rtk cargo test --test realtime_server_function_events_contract -- --list
```

The local `rtk` wrapper suppresses the test-list body, so the generated test
binary was read back with its actual list:

```text
server_function_events::tests::function_call_events: test
values::tests::bounded_text_and_g711_ulaw: test
values::tests::opaque_ids_and_redacted_errors: test

3 tests, 0 benchmarks
```

The exact guarded selector then completed with exit **0** and one matched
test:

```text
rtk cargo test --test realtime_server_function_events_contract server_function_events::tests::function_call_events -- --exact
cargo test: 1 passed, 2 filtered out (1 suite, 0.00s)
```

The full guarded harness completed with exit **0** (`3 passed`). The focused
test covers exact wire tags and fields, correlation IDs and indexes, delta
fragments, completed object validation and bounds, acknowledgement/type
rejection, unknown tags/fields, required-field/null failures, and redacted
errors/debug output.

## Checks

| Command | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 --check src/realtime/server_function_events.rs tests/realtime_server_function_events_contract.rs` | PASS — owned source and harness formatted. |
| `rtk cargo fmt --all -- --check` | **Pre-existing failure** — unrelated differences in `src/pa/fakes/mail.rs` and `src/service.rs`; the owned paths have no formatter differences. |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found. |
| `rtk git diff --check` | PASS. |
| `rtk make check` (first run) | **Environment bootstrap failure** — Rust tests/docs completed, then `docusaurus: command not found` because `website/node_modules` was absent. |
| `rtk make docs-install` | PASS — installed from the checked-in website lockfile; no manifest or lockfile changes. npm reported existing 7 moderate and 17 high advisories; no remediation was performed. |
| `rtk make check` (after docs install) | PASS — exit 0; 535 library tests, integration suites, 3 doc-tests, Clippy, rustdoc, and Docusaurus completed successfully. |

The clean baseline before this package was `rtk cargo test --lib` with 535
passing tests. The library selector for #223 is intentionally not claimed:
the source module remains unregistered by this package and #225 owns the
post-registration library-selector proof.

## Scope, security, and residuals

The event boundary accepts exactly these tags:

- `response.function_call_arguments.delta` with required
  `event_id`, `response_id`, `item_id`, `output_index: u32`, `call_id`, and
  `delta: FunctionArguments`.
- `response.function_call_arguments.done` with those required correlation
  fields plus required `name: String` and completed object-only
  `arguments: FunctionArguments`.
- `conversation.item.created` with required `event_id` and
  `FunctionCallOutputAckItem { id, type, call_id, output }`, where the sole
  item type is `function_call_output`.

All objects reject unknown fields. IDs are required non-null `OpaqueId`
values. Delta arguments check only the shared UTF-8 byte bound, while done
arguments check the bound before accepting one JSON object; original text is
preserved exactly. Tool output is typed inert data and is never executed.

Malformed, unknown, missing, null, unsupported, or oversized input fails with
the shared fixed redacted error classifications. Event and acknowledgement
debug output is redacted. Decoding is deterministic and replay-inert: this
package performs no registration, dispatch, provider, tool, PA, queue, sink,
playback, websocket, filesystem, environment, network, persistence, state,
credential, deployment, or live/UAT action. CI, provider/live behavior,
credentials, deployment, and authenticated UAT were not observed here.

## Delivery readback

The source and harness are separate one-file atomic commits:

- `c7b7992d6c68f9a523552156b1324c051c4de666`
  `feat(realtime): add function-call server events`
- `2fffcc2bd193f5265c959dddb19c643dbd4faaa8`
  `test(realtime): add function-event contract harness`

The report is this separate one-file documentation commit. The delivering PR
must use this footer and must not close parent tracker #97:

```text
Closes #223
Refs #97
Refs #219
Refs #217
```

Rollback is a normal revert of the three package commits in reverse order;
there is no persistent data or provider side effect.
