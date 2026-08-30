# Task 6c1 report: PA service facade and durable availability search

- **Issue:** [#178](https://github.com/djh00t/agent_voice/issues/178)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Package:** `task-6c1`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-task6-build`
- **Branch:** `codex/agent-voice-pa-06-service-layer`
- **Implementation commits:** `bb10f2d`, `1512f9e`

## Scope and ownership

This package owns the first application-service boundary for availability:

- `src/pa/service.rs`: `PaService`, `AvailabilitySearch`, `ServiceError`,
  `search_slots`, and focused service tests.
- `src/pa/mod.rs`: export-only service-module wiring.
- This report.

The package reads both calendar capabilities, applies the existing typed
availability policy, and durably freezes a non-empty quote. It does not own
request preparation or submission, proposals/events, messages, owner tasks,
RSVP reconciliation, HTTP, live provider clients, credentials, configuration
persistence, or new dependencies. Those boundaries remain with later service
packages.

The service borrows `PaStore`, Outlook and Google calendar trait objects, and
explicit `ProviderSession` values. It clones the validated
`AvailabilityPolicy`, so the service's policy cannot change through an
external mutable reference during a search.

## Contract, schema, and workflow

### Request and response schema

```text
search_slots(
    appointment_kind: AppointmentKind,
    now: OffsetDateTime,
    limit: usize,
) -> ServiceResult<AvailabilitySearch>

AvailabilitySearch {
    quote: Quote,                 // opaque, valid for five minutes
    appointment_kind: AppointmentKind,
    timezone: String,             // validated IANA policy timezone
    offered_slots: Vec<AppointmentSlot>, // ordered, frozen UTC intervals
}
```

`limit` is bounded to `1..=100`. The explicit clock is normalized to UTC.
The provider range is the checked half-open interval `[now, now + horizon)`.
The service calls `list_busy` exactly once for each calendar/session, unions
both busy sets through `AvailabilityPolicy`, and persists the exact resulting
slots with `Quote::new(now)`. A successful result is reconstructed from the
stored aggregate rather than from a separately assembled response.

Closed service failures are `InvalidInput`, `Availability`,
`OutlookCalendar`, `GoogleCalendar`, `Store`, and `NoAvailability`. Their
display/debug output is category-only where provider/store errors could carry
sensitive details.

### Workflow

1. Validate the limit and checked time conversions before provider calls.
2. Read Outlook busy intervals, then Google busy intervals, over one shared
   UTC range.
3. Calculate policy-compliant slots from the union of both calendars.
4. Return `NoAvailability` without writing when no slot exists.
5. Persist one non-empty quote and return the stored reconstruction.

The service has no state-changing provider operation. Each invocation creates
a fresh opaque quote; same-quote retry semantics are owned by the later
prepare/submit and store contracts. A provider or store failure leaves no
quote from this operation.

## Acceptance mapping

| Contract / acceptance condition | Evidence in `src/pa/service.rs` | Status |
| --- | --- | --- |
| Either calendar's busy interval blocks the corresponding candidate | `either_calendar_busy_interval_blocks_the_conflicting_start`; `union_busy_intervals_skip_each_calendar_conflict_and_return_the_third_callback_start` | PASS (LOCAL) |
| One read per calendar over the same bounded range | `empty_calendars_return_ordered_literal_starts_up_to_requested_limit`, union test, and fake operation counts | PASS (LOCAL/STATIC) |
| Successful search stores one durable frozen quote | `search_slots_persists_and_returns_the_frozen_quote`; `file_backed_search_quote_survives_service_and_store_reopen` | PASS (LOCAL) |
| Quote preserves kind, timezone, expiry, and ordered slots | successful-search and file-backed reopen assertions | PASS (LOCAL) |
| Invalid limits fail before provider calls or writes | `invalid_limits_do_not_call_either_provider_or_write_a_quote` | PASS (LOCAL) |
| First provider failure stops the workflow; second provider is not called | `outlook_failures_stop_before_google_and_write_no_quote` | PASS (LOCAL) |
| Google failure after Outlook read writes no quote | `google_failures_follow_outlook_and_write_no_quote` | PASS (LOCAL) |
| Store failure after both reads is surfaced without false availability | `store_failure_after_both_reads_returns_closed_store_error` | PASS (LOCAL) |
| Empty availability does not create an empty quote | `no_available_slots_does_not_write_an_empty_quote` | PASS (LOCAL) |
| Horizon/quote time overflow fails closed before provider access | `horizon_and_quote_expiry_overflow_fail_before_provider_calls` | PASS (LOCAL) |
| Sensitive quote, slot, timezone, account, and token values stay out of output | `search_and_service_errors_redact_sensitive_values_from_display_and_debug`; manual `Debug` implementations | PASS (LOCAL/STATIC) |

The issue's Gherkin acceptance is therefore covered by the durable-quote and
both-calendar conflict tests: when either calendar reports a busy candidate,
that candidate is absent from the returned ordered slots and the successful
search has one persisted quote.

## Failure, idempotency, and security review

- Input validation occurs before any calendar call. Invalid limits and checked
  overflow return closed errors and perform zero provider calls and zero
  writes.
- Outlook is read before Google. An Outlook failure prevents the Google read;
  a Google failure follows exactly one Outlook read and still prevents quote
  persistence.
- Store failure happens only after both reads and slot calculation; the
  service returns `ServiceError::Store` and does not claim availability.
- No-availability is distinct from provider and store failure, and does not
  persist an empty aggregate.
- Search retries are intentionally fresh operations with fresh quote identity.
  There is no same-quote state-changing provider retry in this package; later
  prepare/submit packages must use the durable quote and their own
  idempotency contracts.
- The service accepts typed `AppointmentKind`, `OffsetDateTime`, provider
  traits, sessions, and policy values only. Model output cannot select URLs,
  recipients, credentials, or arbitrary provider operations through this
  boundary.
- `AvailabilitySearch` debug output includes only the type and offered-slot
  count; quote ID, slot timestamps, timezone, and provider sessions are
  redacted. `ServiceError` display/debug output exposes only safe categories.
- No email bodies, call transcripts, credentials, access tokens, raw provider
  payloads, or network behavior are introduced by this package.

## RED evidence

No historical failing-first command was captured in this handoff. The issue's
expected RED condition was the absent `PaService::search_slots` facade and
service tests; that is reconstructed context only. The original RED run is
unavailable, and this report does not invent output or a failure count.

**RED status:** unavailable; no fabricated historical evidence.

## GREEN evidence (LOCAL)

The following results were already observed for the implementation worktree.
They are LOCAL evidence only and are not CI or live-provider evidence.

| Check | Command | Result |
| --- | --- | --- |
| Focused service tests | `rtk cargo test --lib pa::service::tests` | PASS — 14 service tests |
| PA library tests | `rtk cargo test --lib pa::` | PASS — 383 PA tests |
| Strict lint | `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| Owned-file formatting | `rtk rustfmt --edition 2024 --check src/pa/service.rs src/pa/mod.rs` | PASS |
| Scoped whitespace/diff review | `rtk git diff --check` | PASS |

The observed tests cover durable persistence/reopen, union and one-call
calendar routing, limit and overflow guards, provider/store failures,
no-availability behavior, and output redaction. No `make check` claim is made
here beyond the commands listed above.

## Review and handoff

The implementation is ready for the package PR review boundary. This report
does not self-approve the code and does not claim remote review or workflow
results.

The next dependency is [#179](https://github.com/djh00t/agent_voice/issues/179)
(`Task 6c2`, prepare request service boundary), which depends on this facade
and the atomic prepare contract from #176. It must load the durable quote,
freeze a selected slot, and produce the exact spoken recap without provider
calls.

## CI, LIVE, and operational non-claims

- **CI:** unverified in this report. A PR check run is required before the
  package can be considered remotely green.
- **LIVE:** unverified and out of scope. No OAuth, Microsoft Graph, Gmail,
  Google Calendar, OpenAI, SIP, deployment, credentials, or production account
  behavior was exercised.
- **Backup/retention/observability:** owned by later packages and not claimed
  here.
- **Rollback:** revert `1512f9e` and `bb10f2d` together if the service facade
  must be withdrawn; no provider-side data is created by these commits.

## PR linkage and completion evidence

The implementation PR must contain the exact closing footer:

```text
Closes #178
```

It must also reference feature #138 and identify commits `bb10f2d` and
`1512f9e`. Issue #178 should remain open until that linked PR is reviewed,
all findings are resolved, CI is green, and the PR is merged.

**Package status:** `status:review` / locally verified; CI and LIVE remain
explicitly unverified.

## Completion evidence record

- **Implementer:** Task 6c1 implementation lane
- **Commits:** `bb10f2d`, `1512f9e`
- **PR:** to be opened with `Closes #178`
- **Commands/results:** listed in the LOCAL evidence table
- **Reviewer:** pending independent review
- **Residual LIVE gates:** provider credentials, OAuth, deployment, and
  end-to-end appointment UAT remain for later integration work
