# Task 6c3c report: reconcile concurrent appointment retries before NoAvailability

- **Issue:** [#316](https://github.com/djh00t/agent_voice/issues/316)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Parent tracker:** [#180](https://github.com/djh00t/agent_voice/issues/180)
- **Follow-up:** [#210](https://github.com/djh00t/agent_voice/issues/210), merged PR [#232](https://github.com/djh00t/agent_voice/pull/232)
- **Package:** `task-6c3c`
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-issue-316`
- **Branch:** `codex/issue-316`
- **Base:** `a20a28b` (`origin/main`)
- **Service commit:** `1d1ca44`

## Scope

This package updates only `PaService::submit_request` and its focused in-file
regression in `src/pa/service.rs`, plus this report. When a fresh submission's
dual-calendar recheck finds the selected interval busy, the service performs
one read-only Google lookup using the application-owned proposal operation key.
The returned event is validated for the exact key, pending title, UTC
interval, timezone, sole configured owner, and `NeedsAction` RSVP before the
existing durable quote, mapping, notification, and audit tail runs. A missing
event remains `NoAvailability`; lookup errors and mismatches fail closed.
No provider create is retried or issued by the recovery path. No public API,
schema, provider trait, dependency, store, fake, migration, HTTP, Realtime,
OAuth, deployment, or owner-task behavior changed.

## TDD RED evidence

After adding the focused test and before the service fix:

```text
cargo test --lib pa::service::tests::matching_keyed_proposal_is_reconciled_before_busy_rejection -- --exact
test ... matching_keyed_proposal_is_reconciled_before_busy_rejection ... FAILED
matching proposal must reconcile: NoAvailability
```

The failure demonstrated that the busy recheck returned before attempting the
operation-key lookup.

## Acceptance evidence (LOCAL)

| Contract | Evidence |
| --- | --- |
| Matching concurrent proposal recovers before busy rejection | `matching_keyed_proposal_is_reconciled_before_busy_rejection` returns pending and records one local proposal, one mapping, one owner notification, and three audits. |
| No duplicate Google create | The same regression observes one pre-seeded matching create and one keyed find; the submit path issues zero second creates. |
| Unrelated busy interval remains unavailable | `newly_busy_slot_fails_before_local_proposal_creation` remains green and verifies zero local proposal, notification, and audit writes. |
| Existing missing-mapping retry remains stable | `exact_retry_repairs_a_missing_mapping_without_duplicate_provider_create` remains green. |
| Provider response validation and fail-closed behavior | Existing `mismatched_provider_event_is_rejected_before_mapping` coverage remains green; the busy recovery path invokes the same pending-event validator before local writes. |
| Bounded read-only recovery | Busy recovery has one `find_proposal` call and returns immediately on `NotFound` or any provider error; it has no retry loop or create fallback. |

## Commands and results

| Check | Result |
| --- | --- |
| `cargo test --lib pa::service::tests::matching_keyed_proposal_is_reconciled_before_busy_rejection -- --exact` (RED) | PASS as a TDD failure: expected `NoAvailability` before the fix |
| `cargo test --lib pa::service::tests::matching_keyed_proposal_is_reconciled_before_busy_rejection -- --exact` (GREEN) | PASS — 1 passed |
| `rtk cargo test --lib pa::service::tests::newly_busy_slot_fails_before_local_proposal_creation -- --exact` | PASS — 1 passed |
| `rtk cargo test --lib pa::service::tests::exact_retry_repairs_a_missing_mapping_without_duplicate_provider_create -- --exact` | PASS — 1 passed |
| `rtk cargo test --lib pa::service::tests` | PASS — 57 passed, 511 filtered out |
| `rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| `rtk git diff --check` | PASS |
| `rtk make check` (first run) | BLOCKED by clean-worktree tooling gap: `docusaurus: command not found` (exit 127); Rust tests/docs completed |
| `rtk make docs-install` | PASS — installed locked website dependencies; npm reported 24 pre-existing audit findings (7 moderate, 17 high) |
| `rtk make check` (rerun) | PASS — Rust tests, clippy, Rust docs, and Docusaurus production build; full Rust run reported 568 tests |

## Security and residual gates

- The lookup is bound to the application-generated operation key and accepts
  no caller or provider-supplied key.
- Every provider field is validated before quote consumption, mapping,
  notification, or audit writes. Provider IDs, tokens, caller data, payloads,
  and raw content are not logged or included in this report.
- Local evidence does not claim CI, live-provider, OAuth, deployment, UAT,
  publication, merge, approval, or tracker closure. Rollback is the local
  service commit revert; no remote deletion or compensation is inferred.
