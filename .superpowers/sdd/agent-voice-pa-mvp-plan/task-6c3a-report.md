# Task 6c3 report: confirmed external request submission orchestration

- **Issue:** [#210](https://github.com/djh00t/agent_voice/issues/210)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Package:** `task-6c3a`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-06c3-submit-orchestration`
- **Branch:** `codex/agent-voice-pa-06c3-submit-orchestration`
- **Base:** `b42498e4e370c1ef4d1e03c98b7361c190ad82fb` (`origin/main`)
- **Implementation commit:** `03f4880`

## Scope

This package adds the typed confirmation boundary and the
`PaService::submit_request` workflow in `src/pa/service.rs`. It reloads and
compares the durable prepared quote/draft, rejects expired or malformed values
before provider mutation, rechecks both calendars, finds or creates one
owner-only pending Google proposal, validates the provider event, persists one
mapping, enqueues the owner notification and the policy-allowed requester
notification, and appends idempotent submission/proposal/outbox audit rows.

`ExplicitConfirmation` has a crate-private constructor and no caller-supplied
boolean, transcript, URL, recipient, credential, or provider payload can mint
the capability. `SubmittedRequest` exposes only durable local IDs and
`ProposalState::Pending`; it does not expose a provider event ID or claim that
the request is booked or accepted.

The ambiguous-create recovery path is deliberately left for #211. A provider
failure after a successful remote create leaves the consumed proposal
unmapped and retryable; the later lookup/recovery package owns that evidence
and behavior.

## RED evidence

Command:

```text
rtk cargo test pa::service::tests::confirmed_submission_creates_one_owner_only_pending_proposal_and_outbox_rows --lib
```

Result at the immutable-main baseline:

```text
cargo test: 2 errors, 0 warnings (1 crates)
error[E0432]: unresolved import `super::ConfirmedPreparedRequest`
error[E0599]: no method named `submit_request`
```

The failure was caused by the absent #210 API, not by a test typo or an
unrelated baseline failure.

## Acceptance evidence (LOCAL)

| Contract | Evidence |
| --- | --- |
| Explicit two-turn capability and redacted values | `ExplicitConfirmation`, `ConfirmedPreparedRequest`, and debug implementations; focused tests construct the capability explicitly. |
| Both-calendar recheck before a new external create | `newly_busy_slot_fails_before_local_proposal_creation`; no local proposal, mapping, notification, or audit is written on busy failure. |
| Expiry fails before provider reads | `expired_prepared_request_fails_before_provider_reads`. |
| One pending Google proposal, mapping, policy notifications, and audits | `confirmed_submission_creates_one_owner_only_pending_proposal_and_outbox_rows`; Meeting produces owner plus requester rows. |
| Callback owner-only policy | `callback_submission_notifies_only_the_owner`; requester ID is `None`. |
| Provider response validation | `mismatched_provider_event_is_rejected_before_mapping`; wrong title/owner is rejected before mapping or tail writes. |
| Local tail retry | `exact_retry_repairs_a_missing_audit_without_provider_calls`; the audit tail failure leaves the valid prefix and retry repairs it without another provider create. |

Commands and results:

| Check | Result |
| --- | --- |
| `rtk cargo test --lib pa::service::tests` | PASS — 33 passed, 446 filtered out |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| `rtk git diff --check` | PASS |
| `rtk make check` | PASS — Rust tests, clippy, docs, and Docusaurus build completed; full suite ran 479 tests |

The first `rtk make check` attempt stopped at `docs-build` because the clean
worktree had no installed Docusaurus binary (`docusaurus: command not found`).
The existing locked website dependencies were installed with
`rtk make docs-install`; no manifest or lockfile was changed. The install
reported 24 existing npm audit vulnerabilities; no audit remediation or
dependency change was made.

## Static and security checks

- Durable quote and draft fields are reloaded and compared before provider
  calls, including quote/draft identity, source ID, caller, kind, UTC range,
  timezone, requester inclusion, idempotency key, and recap.
- Prepared quotes are checked for issuance/expiry before lookup or calendar
  reads; consumed quotes support exact retry after expiry.
- Provider events are checked for operation key, deterministic pending title,
  exact UTC range/timezone, exactly one owner attendee, and `NeedsAction` RSVP.
- Mapping, notification, and audit keys are deterministic and use the store's
  exact-retry semantics. Missing local tail rows are repairable without a
  second create.
- Debug output for confirmation, prepared requests, and submitted results is
  redacted. No provider event ID, token, transcript, raw provider payload, or
  email body is returned by the result or error formatting.

## Non-claims and residual gates

- Evidence is LOCAL only; no CI, live Google/Outlook provider, OAuth,
  deployment, publication, or authenticated UAT evidence is claimed.
- #211 owns ambiguous provider-create recovery and its audit/evidence path;
  it remains unimplemented here.
- Separate focused tests for injected mapping and notification write failures
  remain useful follow-up coverage; the implementation uses the same
  idempotent local-tail mechanism for those rows.
- Rollback is a code revert of `03f4880`; no remote deletion is inferred or
  attempted.

## Completion evidence

- **Implementer:** Codex delegated #210 lane
- **Commit:** `03f4880` (`feat(service): orchestrate confirmed request submission`)
- **Report commit:** added separately after implementation commit
- **PR/push:** not created or pushed, per task instruction
- **Reviewer:** not performed in this lane
