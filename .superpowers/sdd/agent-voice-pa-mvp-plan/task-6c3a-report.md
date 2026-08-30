# Task 6c3 report: confirmed external request submission orchestration

- **Issue:** [#210](https://github.com/djh00t/agent_voice/issues/210)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Package:** `task-6c3a`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-06c3-submit-orchestration`
- **Branch:** `codex/agent-voice-pa-06c3-submit-orchestration`
- **Base:** `4ba837e6ed7f2cd4ba431660865d902a2787f9eb` (`origin/main`)
- **Implementation commits:** `5d2d43e`, `cf72a79`, `5d4efc9`, `82e6723`, `68e8e2e`, `499adc5`, `95f216b`, `748d613`

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

Submission validation retains full-precision UTC for the quote's half-open
`issued_at`/`expires_at` interval. Only after that decision, a separate durable
timestamp is canonicalized to UTC whole seconds for quote consumption and the
notification/audit tail, matching the audit store precision without creating a
partial submission.

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

The regression was then run before the precision fix:

```text
rtk cargo test pa::service::tests::nonzero_nanosecond_submission_retries_without_partial_state --lib
```

It failed with `nonzero nanoseconds must be canonicalized: Store` after the
submission path reached the audit tail.

The fractional issuance boundary was also RED before retaining the full
precision validation instant:

```text
rtk cargo test pa::service::tests::fractional_issued_at_is_valid_at_exact_issue --lib
```

It failed with `exact fractional issue must be valid: Store` because flooring
the instant made it appear earlier than the fractional `issued_at`.

## Acceptance evidence (LOCAL)

| Contract | Evidence |
| --- | --- |
| Explicit two-turn capability and redacted values | `ExplicitConfirmation`, `ConfirmedPreparedRequest`, and debug implementations; focused tests construct the capability explicitly. |
| Both-calendar recheck before a new external create | `newly_busy_slot_fails_before_local_proposal_creation`; no local proposal, mapping, notification, or audit is written on busy failure. |
| Expiry fails before provider reads | `expired_prepared_request_fails_before_provider_reads`. |
| One pending Google proposal, mapping, policy notifications, and audits | `confirmed_submission_creates_one_owner_only_pending_proposal_and_outbox_rows`; Meeting produces owner plus requester rows. |
| Callback owner-only policy | `callback_submission_notifies_only_the_owner`; requester ID is `None`. |
| Provider response validation | `mismatched_provider_event_is_rejected_before_mapping`; wrong title/owner is rejected before mapping or tail writes. |
| Local tail retry | `exact_retry_repairs_a_missing_audit_without_provider_calls`, `exact_retry_repairs_a_missing_mapping_without_duplicate_provider_create`, and `exact_retry_repairs_a_missing_notification_without_provider_calls`; audit, mapping, and notification failures leave valid prefixes whose exact retries converge without a duplicate provider create. |
| Subsecond submission retry | `nonzero_nanosecond_submission_retries_without_partial_state`; a nonzero-nanosecond submission succeeds with a whole-second consumed timestamp, and an exact retry returns the same result without provider calls or duplicate rows. |
| Fractional quote boundaries | `fractional_issued_at_is_valid_at_exact_issue` and `fractional_expires_at_remains_exclusive`; full-precision validation accepts exact issuance and rejects exact fractional expiry. |

Commands and results:

| Check | Result |
| --- | --- |
| `rtk cargo test pa::service::tests::fractional --lib` | PASS — 2 passed, 494 filtered out |
| `rtk cargo test pa::service::tests:: --lib` | PASS — 43 passed, 454 filtered out |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| `rtk git diff --check` | PASS |
| `rtk make check` | PASS — Rust tests, clippy, docs, and Docusaurus build completed; full suite ran 497 tests |

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
- Submission quote validity compares a full-precision UTC `validation_now`;
  only the post-validation durable timestamp is normalized to whole seconds,
  preserving both fractional boundaries and audit precision.
- Provider events are checked for operation key, deterministic pending title,
  exact UTC range/timezone, exactly one owner attendee, and `NeedsAction` RSVP.
- Mapping, notification, and audit keys are deterministic and use the store's
  exact-retry semantics. Missing local tail rows are repairable without a
  second create.
- Debug output for confirmation, prepared requests, and submitted results is
  redacted. No provider event ID, token, transcript, raw provider payload, or
  email body is returned by the result or error formatting.

## Reviewer P1 remediation

Durable appointment-draft start and end values are now rejected unless both
use the UTC offset before proposal lookup, calendar reads/creates, quote
consumption, mapping, notification, or audit writes. This closes the gap where
instant equality accepted a non-UTC durable RFC 3339 value and
`NotificationTemplateData` rejected it only after earlier submission effects.

`non_utc_durable_interval_fails_before_submission_side_effects_and_exact_retry_converges`
corrupts both durable values to an equivalent `+10:00` interval, verifies no
proposal lookup/create or local submission row, restores the exact UTC values,
then verifies the initial valid submission and exact retry converge to one
proposal and one owner-only notification/audit tail.

Focused command:

```text
rtk cargo test pa::service::tests::non_utc_durable_interval_fails_before_submission_side_effects_and_exact_retry_converges --lib
```

Result: PASS — 1 passed, 495 filtered out.

Current repository gate:

```text
rtk make check
```

Result: PASS — 496 unit tests, 3 doctests, strict clippy, Rust API docs, and
the Docusaurus production build. The fresh worktree first lacked the local
Docusaurus binary; `rtk make docs-install` installed the lockfile-resolved
website dependencies without changing tracked manifests or lockfiles.

## Reviewer local-tail remediation

The service owns three independently retryable local tail boundaries. Focused
SQLite trigger regressions now inject failures before mapping insertion and
notification enqueue, in addition to the existing audit insertion failure.
The mapping retry is allowed to find the already-created operation-key event,
but provider busy reads and proposal creation are forced to fail and are not
repeated. The notification retry forces every provider operation to fail; it
still repairs the notification and audit rows because its existing valid
mapping bypasses all provider calls.

Focused command:

```text
rtk cargo test pa::service::tests::exact_retry_repairs_a_missing_ --lib
```

Result: PASS — 3 passed, 493 filtered out.

## Reviewer owner-binding remediation

Each event mapping now persists a SHA-256 fingerprint of the owner address in
its redacted source identity. The existing-mapping tail-repair path recomputes
that identity from the configured owner before notification enqueue. A changed
owner therefore returns a redacted mapping conflict before provider calls or
local notification writes; an unchanged owner retains the provider-free tail
repair path.

`owner_change_after_mapping_fails_closed_without_provider_calls_or_misrouting`
injects a notification failure after the original owner's mapping succeeds,
reconstructs the service with a replacement owner, forces each provider
operation to fail, and verifies the retry fails closed with no notification,
calendar lookup, busy read, or duplicate create.

Focused command:

```text
rtk cargo test pa::service::tests::owner_change_after_mapping_fails_closed_without_provider_calls_or_misrouting --lib
```

Result: PASS — 1 passed, 495 filtered out.

## Reviewer meeting-buffer remediation

Fresh submissions now expand the selected durable interval by the configured
meeting buffer with checked subtraction/addition, query both calendars over
that expanded half-open range, and reject any overlap before proposal lookup,
creation, mapping, or notification writes. Expansion overflow fails closed
before provider calls. A retry with an already persisted proposal but no
mapping keeps its existing operation-key recovery path: rechecking it would
see the proposal's own busy event and prevent mapping-tail repair.

`submission_recheck_enforces_pre_and_post_buffer_but_zero_buffer_is_unchanged`
verifies events only inside the pre- and post-buffers block a fresh submission,
while the same pre-slot event is ignored under a zero buffer.
`submission_buffer_expansion_overflow_fails_before_provider_calls` verifies a
representable maximal policy buffer fails before proposal lookup or busy reads.

Focused command:

```text
rtk cargo test pa::service::tests::submission_ --lib
```

Result: PASS — 2 passed, 494 filtered out.

## Reviewer unmapped-retry remediation

When a durable proposal exists but operation-key lookup returns `NotFound`, the
service now repeats the checked expanded-buffer dual-calendar recheck directly
before the impending create. Existing-event recovery remains unchanged.
`unmapped_proposal_not_found_rechecks_buffer_before_retry_create` leaves a
durable proposal after a closed create failure, adds an unrelated event only in
the pre-buffer, then verifies the retry fails before another create or local
tail write.

Focused command:

```text
rtk cargo test pa::service::tests::unmapped_proposal_not_found_rechecks_buffer_before_retry_create --lib
```

Result: PASS — 1 passed, 496 filtered out.

## Non-claims and residual gates

- Evidence is LOCAL only; no CI, live Google/Outlook provider, OAuth,
  deployment, publication, or authenticated UAT evidence is claimed.
- #211 owns ambiguous provider-create recovery and its audit/evidence path;
  it remains unimplemented here.
- Rollback is a code revert of `5d2d43e`, `cf72a79`, `5d4efc9`, `82e6723`,
  `68e8e2e`, `499adc5`, `95f216b`, and `748d613`; no remote deletion is inferred or attempted.

## Completion evidence

- **Implementer:** Codex delegated #210 lane
- **Commits:** `5d2d43e` (`feat(service): orchestrate confirmed request submission`),
  `cf72a79` (`fix(service): canonicalize submission audit time`), `5d4efc9`
  (`fix(service): preserve fractional quote boundaries`), `82e6723`
  (`fix(service): reject non-UTC durable submission intervals`), `68e8e2e`
  (`test(service): cover local submission tail repair`), `499adc5`
  (`fix(service): bind mapping retries to proposal owner`), `95f216b`
  (`fix(service): recheck submission meeting buffers`)
- **Report commit:** added separately after reviewer remediation
- **PR/push:** not created or pushed, per task instruction
- **Reviewer:** not performed in this lane
