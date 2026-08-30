# Task 6c3b report: ambiguous provider recovery and submission audit evidence

- **Issue:** [#211](https://github.com/djh00t/agent_voice/issues/211)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Parent tracker:** [#180](https://github.com/djh00t/agent_voice/issues/180)
- **Package:** `task-6c3b`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-06c3b-ambiguous-recovery`
- **Branch:** `codex/agent-voice-pa-06c3b-ambiguous-recovery`
- **Base:** `76e8df4` (`origin/main`, current after PR #242)
- **Test commits:** `fbda37c`, `ee534f5`

## Scope

This package adds one end-to-end regression test in
`src/pa/fakes/calendar.rs`. The test consumes the merged #210
`PaService::submit_request` contract and the existing fake
`queue_ambiguous_create_failure` hook. It does not modify service, provider,
or store production code. Rustfmt also normalized two pre-existing formatting
hunks in this same owned file.

The test uses a Meeting request so the policy-allowed owner and requester
notification paths are both exercised. All evidence below is local and uses
closed categories, counts, and immutable-key behavior only; no provider event
ID, operation key, caller data, token, or raw provider response is recorded in
this report.

## TDD RED evidence

At the initial immutable-main baseline (`a559829`), before adding the test:

```text
rtk cargo test pa::fakes::calendar::tests::google_ambiguous_create_is_recovered_by_operation_lookup --lib
cargo test: 0 passed, 497 filtered out (1 suite, 0.00s)
```

The focused test was absent, so the baseline had no queued-ambiguity evidence.
After the test was added, the same command passed as recorded below.

## Acceptance evidence (LOCAL)

| Contract | Evidence |
| --- | --- |
| Remote-side effect followed by a closed error | The test queues one `ProviderError::Unavailable`; the first confirmed submission returns the redacted Google-calendar category after the fake has materialized the event. |
| Pre-create availability recheck and confirmation boundary | The confirmed consumer path performs two busy reads per calendar (search plus submission recheck) before the first create; the test must cross `ConfirmedPreparedRequest`. |
| One remote event and no false local success | After the first error, fake state contains one proposal draft, one event, and one change; local proposal count is `1`, while mapping, notification, and audit counts are all `0`. |
| Operation-key lookup recovery | The exact durable retry finds the materialized event, validates its title, interval, timezone, sole owner, and `NeedsAction` RSVP, and finishes without another create. Counts are `CalendarProposalCreate = 1` and `CalendarProposalFind = 2` after recovery (the first submission lookup plus the retry lookup). |
| Local convergence | After recovery, local counts are proposals `1`, mappings `1`, notifications `2`, and audits `4`; both outbox rows point to the recovered proposal and mapping. |
| Stable exact retry | A third exact submission returns the same `SubmittedRequest`; all local counts and provider operation counts remain unchanged. |
| Immutable audit tail | The four audit rows remain in the expected request/proposal/owner-notification/requester-notification sequence. Every row is reloaded by idempotency key before and after the exact retry and compared as the original `StoredAuditEvent`; repeating the submission does not add rows or alter immutable content. |
| Redaction | The first service error has category-only Display/Debug output. Fake Debug exposes only counts and omits event identity, operation identity, title, owner data, and session token. The store audit contract is details-free. |

## Commands and results

| Check | Result |
| --- | --- |
| `rtk cargo test pa::fakes::calendar::tests::google_ambiguous_create_is_recovered_by_operation_lookup --lib` | PASS — 1 passed, 507 filtered out |
| `rtk cargo test pa::service::tests::confirmed_submission_creates_one_owner_only_pending_proposal_and_outbox_rows --lib` | PASS — 1 passed, 507 filtered out |
| `rtk cargo test pa::service::tests::exact_retry_repairs_a_missing_ --lib` | PASS — 3 passed, 505 filtered out |
| `rtk cargo test pa::store::tests::audit_append_retries_stably_and_conflicting_keys_preserve_the_original --lib` | PASS — 1 passed, 507 filtered out |
| `rtk cargo test pa::service::tests::mismatched_provider_event_is_rejected_before_mapping --lib` | PASS — 1 passed, 507 filtered out |
| `rtk cargo test pa:: --lib` | PASS — 437 passed, 71 filtered out |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk rustfmt --edition 2024 --check src/pa/fakes/calendar.rs` | PASS |
| `rtk git diff --check` | PASS |
| `rtk git diff --check origin/main...HEAD` | PASS — no whitespace errors in the reviewed range |
| `rtk cargo doc --no-deps` | PASS |
| `rtk make check` | PASS — 508 tests, clippy, Rust docs, and Docusaurus build |

The first `rtk make check` attempt stopped because the clean worktree lacked
the existing website binary (`docusaurus: command not found`, exit 127).
`rtk make docs-install` installed the locked website dependencies without
changing tracked files; npm reported 24 existing audit vulnerabilities (7
moderate, 17 high). The rerun of `rtk make check` passed.

## Static and security review

- The fake persists the valid proposal before dequeuing the queued closed
  error, retains the immutable create draft, and finds only by the exact
  operation key.
- The consumer validates the recovered event before mapping or notification
  side effects. A wrong operation key, title, interval, timezone, owner, or
  RSVP remains fail-closed under the existing #210 test coverage.
- The first ambiguous failure leaves only the valid local proposal prefix;
  the retry repairs mapping, outbox, and audit rows idempotently. A second
  retry is provider-free once the mapping exists.
- Audit rows contain closed event/entity kinds and opaque local IDs only. No
  audit details, provider payload, event identity, caller content, token, or
  transcript is emitted by the test or report.

## Residual gates

No CI, live-provider, OAuth, deployment, publication, or UAT claim is made.
Rollback is the local commit revert; no remote event deletion or manual
compensation is inferred. Push, PR publication, merge, and approval remain
outside this package.
