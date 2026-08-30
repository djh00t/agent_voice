# Task 6c2 report: provider-free appointment request preparation

- **Issue:** [#179](https://github.com/djh00t/agent_voice/issues/179)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Package:** `task-6c2`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-task6-build`
- **Branch:** `codex/agent-voice-pa-06-service-layer`
- **Implementation commit:** `3ed5bff`

## Prerequisite evidence

- Issue #176's atomic quote-to-draft prepare boundary is implemented by
  `40439ac` in green, mergeable PR #204, which now contains `Closes #176`.
- Issue #178's service facade is implemented by `05dc49c` and exported by
  `781f7e6`; its fresh-review findings were remediated by `d0b6fc3` and the
  synchronized Task 6c1 evidence update.

This stack remains ordered after PR #204 and the Task 6c1 commits.

## Scope and ownership

This package adds the provider-free preparation boundary after availability
search. It loads an immutable, durable quote, selects one already-offered
slot, freezes the caller's appointment request, and returns the exact recap
that a voice runtime can read back before a later submission step.

This package owns `PreparedRequest`, `PaService::prepare_request`, deterministic
recap formatting, and the service-level mapping of domain/store failures. The
atomic quote-to-draft transition, persistence, immutable retry behavior, and
quote validity checks remain implemented by the existing `PaStore` contract.

It does not call Outlook, Google Calendar, Gmail, Microsoft Graph, OpenAI,
SIP, or any other provider. It does not submit a request, create an event,
send a notification, or decide whether a spoken response is affirmative. The
later submission package owns those operations and the second-turn
confirmation gate.

## Contract and schemas

### `prepare_request` service contract

```text
PaService::prepare_request(
    quote_id: QuoteId,
    slot_index: u32,
    caller: CallerIdentity,
    expected_kind: AppointmentKind,
    requester_included: Option<bool>,
    source_id: impl AsRef<str>,
    idempotency_key: IdempotencyKey,
    now: OffsetDateTime,
) -> ServiceResult<PreparedRequest>
```

Inputs are typed and validated by the domain/store boundary. `quote_id` must
identify the durable quote, `slot_index` must identify one frozen offered
slot, `caller` contains the validated name and confirmed email,
`expected_kind` must match the quote, and `source_id` plus
`idempotency_key` identify the originating call and logical operation.
`requester_included: None` selects the appointment-kind default; an explicit
boolean is an allowed caller choice. The explicit `now` is normalized to UTC
before store validation.

### `PreparedRequest` result schema

```text
PreparedRequest {
    draft_id: i64,
    quote_id: QuoteId,
    caller: CallerIdentity,
    kind: AppointmentKind,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    timezone: String,
    requester_included: bool,
    recap: String,
}
```

The fields are private and are exposed only through deliberate accessors:
`draft_id`/`appointment_draft_id`, `quote_id`, `caller`/`caller_name`/
`caller_email`, `kind`/`appointment_kind`, `starts_at`/
`selected_starts_at`, `ends_at`/`selected_ends_at`, `timezone`,
`requester_included`/`includes_requester`, and `recap`/`spoken_recap`.
The result is reconstructed from the stored aggregate, so callers receive the
immutable persisted choice rather than an untrusted proposed value.

### Exact recap schema

The recap is generated from the stored draft and stored validated IANA
timezone using this exact format:

```text
{Kind} for {caller.name} <{caller.email}> on {YYYY-MM-DD} at {HH:MM} ({IANA timezone}); {requester included|requester not included}.
```

`Kind` is `Callback` or `Meeting`. The stored UTC start is converted to the
stored timezone before rendering the date and 24-hour local time. For
example, the contract test expects:

```text
Meeting for Ada Lovelace <ada@example.test> on 2026-09-01 at 08:00 (Australia/Sydney); requester included.
```

The recap is an explicit application value for the voice layer. It is not an
authorization or submission result; until the later submit boundary succeeds,
the assistant must describe the request as requested, not booked.

## Provider-free workflow

1. Load the quote by `quote_id` from `PaStore`.
2. Reject a mismatched `expected_kind` before constructing a draft.
3. Select `slot_index` from the quote's frozen ordered slots.
4. Resolve requester inclusion from the explicit override or the kind default
   (`Callback` false, `Meeting` true).
5. Construct an `AppointmentDraft` from the selected slot start, deriving its
   end from the appointment kind, and bind the quote ID and idempotency key.
6. Call the store's atomic
   `prepare_appointment_draft_from_quote` operation with the source ID and
   normalized `now`.
7. Let the store enforce the issued/prepared/consumed lifecycle, quote
   validity interval, immutable field equality, and one-winner transaction.
8. Rebuild `PreparedRequest` from the stored draft and quote timezone, then
   compute the exact local-time recap.

The durable-quote test explicitly proves the provider-free boundary: it
preloads one quote, calls `prepare_request`, and asserts a literal **zero calendar calls**
result for both Outlook and Google (`CalendarBusy` invocation count is `0` for
each). No provider session or credential can influence this operation.

## Acceptance and evidence mapping

| Contract / acceptance condition | Evidence in `src/pa/service.rs` | Status |
| --- | --- | --- |
| A frozen quote can prepare one selected appointment request | `prepare_request_returns_spoken_recap_for_frozen_slot_without_provider_calls` | PASS (LOCAL) |
| Result preserves draft ID, quote ID, kind, exact slot, timezone, and inclusion choice | Same test's field assertions | PASS (LOCAL) |
| Recap uses the stored IANA timezone and exact deterministic wording | Same test's exact recap assertion | PASS (LOCAL) |
| Callback defaults to owner-only and Meeting defaults to requester-included | `prepare_request_applies_inclusion_defaults_and_explicit_override` | PASS (LOCAL) |
| Explicit inclusion override is preserved in the draft and recap | `prepare_request_applies_inclusion_defaults_and_explicit_override` | PASS (LOCAL) |
| Preparation performs no Outlook or Google Calendar calls | Durable-quote test's literal `0`/`0` `CalendarBusy` assertions; retry and invalid-input tests also assert `0`/`0` | PASS (LOCAL) |
| Exact retries remain stable, including after quote expiry | `prepare_request_retries_exactly_after_expiry_and_conflicts_on_changes` | PASS (LOCAL) |
| Changing slot, caller, inclusion, source, idempotency key, or kind conflicts | `prepare_request_retries_exactly_after_expiry_and_conflicts_on_changes` | PASS (LOCAL) |
| Unknown quote, invalid slot, expired quote, and not-yet-valid quote fail closed | `prepare_request_rejects_unknown_invalid_and_temporally_invalid_quotes` | PASS (LOCAL) |
| Caller identity and recap are not exposed by ordinary debug output | `prepared_request_debug_redacts_caller_and_spoken_recap` | PASS (LOCAL) |
| Provider/store error text and availability debug output remain redacted | `search_and_service_errors_redact_sensitive_values_from_display_and_debug` | PASS (LOCAL) |

## Failure, idempotency, and security mapping

- Quote lookup failures are returned as closed `ServiceError::Store` results;
  no provider operation is attempted.
- A kind mismatch, invalid slot, invalid source, or invalid draft identity is
  rejected without creating an orphan draft.
- The store's half-open validity interval rejects a quote before issuance and
  at or after expiry for a first preparation. Exact retries of an already
  prepared or consumed aggregate return the unchanged stored result even when
  the retry's `now` is after expiry.
- Any changed immutable field—slot, caller, kind, inclusion, source, or
  idempotency key—returns a conflict and leaves the aggregate unchanged.
- The store transaction binds the draft and quote state together; concurrent
  attempts cannot produce two prepared winners through this service boundary.
- The application accepts typed caller, quote, appointment, source, and
  idempotency values only. No model-provided URL, recipient, credential,
  provider operation, or policy can enter this boundary.
- `PreparedRequest`'s `Debug` implementation redacts quote identity, caller,
  timestamps, timezone, and recap. `ServiceError` display/debug output uses
  category-only values, and no complete email body, transcript, token, or raw
  provider payload is introduced.
- The recap accessor intentionally returns the exact spoken content to the
  trusted voice application; logging the ordinary debug representation does
  not expose it.

## RED evidence

No historical failing-first command was captured in this handoff. The expected
RED condition is reconstructed context: before `3ed5bff`, the service lacked
the `PreparedRequest` and `PaService::prepare_request` boundary and its
provider-free preparation tests. The original RED run is unavailable; this
report does not invent output or a failure count.

**RED status:** unavailable; reconstructed only.

## GREEN evidence (LOCAL)

The following evidence is local to the implementation worktree and is not a
claim about CI or live providers.

| Check | Command | Result |
| --- | --- | --- |
| Focused service tests | `rtk cargo test --lib pa::service::tests` | PASS — 19 service tests |
| Strict lint | `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| Owned-file formatting | `rtk rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| Scoped whitespace/diff review | `rtk git diff --check` | PASS |

The focused tests cover the exact recap, inclusion defaults and override,
zero provider calls, quote expiry/retry/conflict behavior, invalid quote
handling, and redacted output. No `make check` result is claimed here.

## Handoff and dependencies

The implementation commit is `3ed5bff` (`feat(service): prepare requests from
frozen quotes`). The next dependency is [#180](https://github.com/djh00t/agent_voice/issues/180),
the submit-request boundary. It must consume this immutable prepared draft,
recheck both calendars during submission, and preserve the two-turn explicit
confirmation rule before any provider-side proposal is created.

The implementation PR must contain the exact closing footer:

```text
Closes #179
```

It must also reference feature #138 and commit `3ed5bff`. Issue #179 should
remain open until the linked PR is reviewed, findings are resolved, CI is
green, and the PR is merged.

## CI, LIVE, and operational non-claims

- **CI:** unverified in this report. A PR check run is required before remote
  completion can be claimed.
- **LIVE:** unverified and out of scope. No OAuth, Microsoft Graph, Gmail,
  Google Calendar, OpenAI, SIP, deployment, credential, or production-account
  behavior was exercised.
- **Provider behavior:** no calendar event, proposal, message, notification,
  or external side effect is created by this package.
- **Backup, retention, polling, metrics, and admin UI:** owned by later
  packages and not claimed here.
- **Rollback:** revert `3ed5bff` if the preparation boundary must be
  withdrawn; this commit makes no provider-side changes.

**Package status:** `status:review` / locally verified; CI and LIVE remain
explicitly unverified.
