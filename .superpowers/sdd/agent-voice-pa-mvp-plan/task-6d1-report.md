# Task 6d1 report: record a redacted voice-message summary and owner notification

- **Issue:** [#181](https://github.com/djh00t/agent_voice/issues/181)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Package:** `task-6d1`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-task6-build`
- **Branch:** `codex/agent-voice-pa-06-service-layer`
- **Implementation commit:** `f498999`

## Prerequisite evidence

This package builds on the Task 6c1 application-service facade from [#178](https://github.com/djh00t/agent_voice/issues/178), implemented in the same service-layer stack. The message path uses the accepted typed store boundaries for:

- structured message values and message record/read operations ([#150](https://github.com/djh00t/agent_voice/issues/150), [#151](https://github.com/djh00t/agent_voice/issues/151));
- notification template and enqueue operations ([#145](https://github.com/djh00t/agent_voice/issues/145)); and
- closed audit values and append-only audit operations ([#148](https://github.com/djh00t/agent_voice/issues/148), [#149](https://github.com/djh00t/agent_voice/issues/149)).

Those store primitives are present in the local persistence stack and provide
the validated, idempotent operations consumed here. Their delivery PR and
merge status remain separate integration gates; this report does not claim
that any prerequisite has been merged.

## Scope and ownership

This package owns the application-service boundary for recording one accepted
voice-call summary. It adds `PaService::with_owner`,
`PaService::record_message`, the redacted `RecordedMessage` result, and five
focused service tests in `src/pa/service.rs`.

It does not own transcript ingestion, speech-to-text, caller recognition,
calendar or Gmail calls, provider adapters, HTTP routes, notification
delivery, classification, or raw call-transcript retention. The service only
queues a typed owner-only notification in the local outbox; a later worker is
responsible for delivery.

## Schemas and contracts

### Owner configuration

```text
PaService::with_owner(
    store,
    outlook_calendar,
    outlook_session,
    google_calendar,
    google_session,
    availability_policy,
    owner: MailAddress,
) -> PaService
```

`MailAddress` is validated before it is injected. The existing
`PaService::new` constructor intentionally has no owner and therefore cannot
record a message. `record_message` returns `OwnerNotConfigured` before any
write when the owner address is absent.

### Message recording API

```text
record_message(
    summary: MessageSummary,
    source_id: validated stable call identity,
    received_at: OffsetDateTime,
) -> ServiceResult<RecordedMessage>

RecordedMessage {
    message_id: i64,       // accessor only; redacted by Debug
    notification_id: i64,  // accessor only; redacted by Debug
}
```

`MessageSummary` is the only message-content input. It is a validated,
structured, bounded value that rejects blank/control-bearing input and has no
raw transcript or complete email-body representation. The persisted message
uses `MessageProvider::Voice`, the validated `source_id`, a derived provider
message identity, the summary, and `subject = None` / `sender = None`.

The owner notification is `NotificationKind::CallSummary`, addressed only to
the configured owner, with a structured title containing the validated summary
and no free-form body, transcript, token, or provider payload.

Two details-free audit rows are appended:

```text
MessageRecorded       -> entity Message       -> stored message database ID
NotificationEnqueued  -> entity Notification  -> stored notification database ID
```

The `RecordedMessage` accessors are available to trusted application code,
but ordinary formatting deliberately renders:

```text
RecordedMessage { message_id: <redacted>, notification_id: <redacted> }
```

## Identities and timestamp canonicalization

One validated source identity produces a private `pa-voice-*` namespace:

```text
pa-voice-message-recorded-{source_id}
pa-voice-provider-message-{source_id}
pa-voice-call-summary-notification-{source_id}
pa-voice-message-recorded-audit-{source_id}
pa-voice-notification-enqueued-audit-{source_id}
```

Every derived identity is validated before the first database write. This
prevents an overlong or malformed source from leaving a durable prefix and
prevents the voice flow from colliding with submit-flow notification/audit
keys. The namespace isolation test proves that owner and requester submit
identities can coexist with voice-message identities.

`received_at` is normalized to UTC and truncated to whole seconds before it is
passed to the store. The message and audit repositories therefore receive the
same canonical instant and RFC3339 whole-second representation. A retry with
nanoseconds in the first call and the equivalent canonical second in the next
call is an exact retry, not a changed message.

## Idempotent message-notification-audit workflow

The local workflow is deliberately an ordered durable prefix:

1. Validate the configured owner, source, all derived identities, summary, and
   canonical timestamp before writing.
2. Record one `MessageProvider::Voice` message through the store's immutable
   identity contract.
3. Enqueue one owner-only `CallSummary` notification.
4. Append the `MessageRecorded` audit event.
5. Append the `NotificationEnqueued` audit event.
6. Return the two durable IDs in `RecordedMessage`.

Each store operation is independently idempotent. Exact same-source retries
return the existing message, notification, and audit rows and converge on one
message, one outbox row, and two audits. The service does not wrap the three
repository calls in one monolithic transaction; instead, a retry repairs any
valid durable prefix left by a failed tail write.

The focused tail-repair test injects a failure at each of the notification,
message-audit, and notification-audit writes. After the trigger is removed,
the exact retry completes the missing suffix without duplication.

## Acceptance mapping

| Contract / acceptance condition | Evidence in `src/pa/service.rs` | Status |
| --- | --- | --- |
| One accepted voice summary creates one persisted voice message | `record_message_persists_one_voice_summary_and_owner_notification` | PASS (LOCAL) |
| The notification is owner-only and uses the call-summary template | `record_message_persists_one_voice_summary_and_owner_notification` | PASS (LOCAL) |
| Exactly two correct audits are appended | `record_message_persists_one_voice_summary_and_owner_notification`; retry and tail tests | PASS (LOCAL) |
| Raw transcripts and complete bodies cannot cross the boundary | `MessageSummary`/structured template contract; no transcript/body fields or accessors | PASS (STATIC/LOCAL) |
| Whole-second UTC canonicalization makes equivalent retries exact | `record_message_persists_one_voice_summary_and_owner_notification`; `record_message_exact_retry_is_stable_and_changed_inputs_conflict` | PASS (LOCAL) |
| Exact retries return stable durable results | `record_message_exact_retry_is_stable_and_changed_inputs_conflict` | PASS (LOCAL) |
| Changed summary, timestamp, or source identity fails closed without overwriting | `record_message_exact_retry_is_stable_and_changed_inputs_conflict` | PASS (LOCAL) |
| Owner absence fails before any message/outbox write | `record_message_owner_is_required_before_any_write_and_debug_is_redacted` | PASS (LOCAL) |
| Voice identities cannot collide with submit-flow identities | `record_message_namespaces_submit_flow_identities` | PASS (LOCAL) |
| Any failed tail can be repaired by one exact retry | `record_message_tail_failures_preserve_prefix_and_retry_to_complete_state` | PASS (LOCAL) |
| Result IDs and error output remain redacted | `record_message_owner_is_required_before_any_write_and_debug_is_redacted` and `RecordedMessage` `Debug` implementation | PASS (LOCAL/STATIC) |
| No calendar operation is made by message recording | Each focused test asserts literal zero for all calendar fake operations | PASS (LOCAL) |

The package therefore covers the issue's acceptance scenario: an accepted
voice summary with a stable source, including a retry after an audit-tail
failure, converges to one message, one owner notification, and two correct
audits without storing a raw transcript.

## Failure, idempotency, and security review

- Missing owner configuration fails before source validation or any write.
- Invalid source or derived identity fails before the first write; all
  namespaces are checked against the store's identifier bounds.
- A changed summary or canonical receive time for an existing source returns a
  generic message conflict and leaves the original row unchanged.
- The owner-only recipient is validated through `MailAddress` and
  `NotificationRecipient`; callers cannot supply a recipient through this
  service method.
- Notification and audit tail failures retain only the valid prefix. Exact
  retries are safe, while changed retries remain conflicts.
- Summary, recipient, source, provider-message identity, and audit identity
  values are absent from `RecordedMessage`'s ordinary debug output. Store
  value types and service errors use redacted/category-only formatting.
- Calendar sessions are borrowed only because this facade also owns
  availability; `record_message` never invokes a calendar capability. The
  five focused tests assert zero calls for busy, sync, owner-create/find,
  proposal-create/find, promote, and delete operations.
- No credentials, tokens, URLs, recipients selected by model output, raw
  email bodies, call transcripts, or provider payloads are accepted or
  persisted by this boundary.

## RED evidence

No historical failing-first output was captured in this handoff. The expected
RED condition is reconstructed context: before `f498999`,
`PaService::record_message` and its five focused tests were absent. The issue's
failing-first command was expected to fail because that service symbol did not
exist, but the original command output is unavailable. This report does not
invent a failure count or claim a historical RED run.

**RED status:** unavailable; reconstructed only.

## GREEN evidence (LOCAL)

The following checks were freshly observed in the implementation worktree.
They are LOCAL evidence only and are not claims about CI, merge, deployment,
or live providers.

| Check | Command | Result |
| --- | --- | --- |
| Focused message tests | `rtk cargo test --lib pa::service::tests::record_message` | PASS — 5 passed, 455 filtered out |
| Full service test module | `rtk cargo test --lib pa::service::tests` | PASS — 24 passed, 436 filtered out |
| Strict lint | `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| Owned-file formatting | `rtk rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| Scoped whitespace/diff review | `rtk git diff --check` | PASS |

The focused tests cover owner-first validation, message/outbox/audit
materialization, timestamp normalization, exact and changed retries,
namespace isolation, tail repair, output redaction, and zero calendar calls.

## Handoff and rollback

The implementation commit is `f498999` (`feat(service): record redacted
voice-message summaries`). The next service-layer work can consume the
`RecordedMessage` result when wiring voice runtime message capture; it must
keep transcript ingestion and notification delivery outside this persistence
boundary.

The implementation PR must contain the exact closing footer:

```text
Closes #181
```

It must also reference feature #138 and prerequisite #178. Issue #181 should
remain open until the linked PR is reviewed, all findings are resolved, CI is
green, and the PR is merged.

Rollback is a code revert of `f498999` and removal of calls to
`record_message`. The commit makes no calendar, Gmail, network, or provider
side effect; existing local message/outbox/audit rows are not destructively
removed by rollback and remain available for a controlled recovery decision.

## CI, LIVE, and operational non-claims

- **CI:** unverified in this report. A remote PR check run is required before
  CI completion can be claimed.
- **LIVE:** unverified and out of scope. No Outlook, Gmail, Google Calendar,
  Microsoft Graph, OpenAI, SIP, OAuth, deployment, credentials, or production
  account behavior was exercised.
- **Delivery:** the notification is queued in the local outbox only; this
  package does not claim that an email was sent or received.
- **Retention, backup, polling, metrics, admin UI, and HTTP:** owned by later
  packages and not claimed here.

**Package status:** `status:review` / locally verified; CI, merge, deployment,
and LIVE remain explicitly unverified.

## Completion evidence record

- **Implementer:** Task 6d1 service lane
- **Commit:** `f498999`
- **PR:** to be opened with `Closes #181`
- **Commands/results:** listed in the LOCAL evidence table
- **Reviewer:** pending independent review
- **Residual LIVE gates:** provider credentials, OAuth, delivery worker,
  deployment, and end-to-end call/message UAT
