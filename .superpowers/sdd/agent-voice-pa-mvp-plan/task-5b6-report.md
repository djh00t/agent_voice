# Task 5b6 evidence: cross-provider fake contract matrix

## Scope

The public fake-only contract matrix composes the calendar, mail, structured
triage, and encrypted-backup provider traits through trait objects. It covers
typed success paths, closed provider failures, cursor recovery, durable
idempotency, concurrent clones, redacted `Debug`, and `Send` futures. The
matrix is implemented in `src/pa/fakes/contract_tests.rs`; this report is the
package's evidence record.

## Contract matrix

| Boundary | Happy path covered | Failure/retry coverage | Idempotency/concurrency coverage | Redaction/`Send` coverage |
| --- | --- | --- | --- | --- |
| Outlook calendar | Busy/read, owner-only create, owner lookup | `TokenExpired`, throttled, unavailable, persistent failure and recovery | Owner event create returns one shared result across clones | Trait future and fake are `Send + Sync`; IDs/titles are absent from debug |
| Google calendar | Read, proposal create, proposal lookup, accepted promotion, delete | Same closed failures with no mutation; malformed cursor and partial/zero-success pages | Create, promote, and delete each share one durable result across concurrent clones | Event IDs, titles, cursors, and attendee addresses are redacted |
| Outlook mail | Incremental mail read | Cursor expiry, partial page, zero-success retry, injected failure | Shared cursor state is exercised through the incoming-mail trait object | Message IDs, addresses, subjects, and bodies do not appear in debug |
| PA Gmail | Read, label mutation, send | Label/send failures preserve state; persistent failure clears and recovers | Label and send operations are exact/idempotent across clones | Outbound and returned mail values are redacted |
| Structured triage | `Actionable`, `Ambiguous`, and `Ignore` decisions | Closed failures, exact invocation counts, no fixture mutation | Repeated and concurrent classification returns one durable decision | Input/message fields and decisions are checked through redacted debug |
| Encrypted backup | Encrypted snapshot upload and receipt | Closed failures preserve stored receipts and provider version sequence | Upload returns one receipt/object across concurrent clones and retries | Ciphertext, checksum, object key, metadata, and receipt fields are redacted |

The common failure helper exercises every state-changing operation with
`TokenExpired`, a typed throttled error, and `Unavailable`, then checks the
exact invocation count, unchanged public state, and recovery after clearing a
persistent failure. `find_owner_event` and `find_proposal` are explicitly
included in this matrix after review identified that lookup operations needed
the same failure/recovery coverage as their corresponding mutations.

The cursor helper covers calendar and both incoming-mail fakes. It verifies
`CursorExpired`, a successful prefix plus item-level failure, retry from the
returned cursor, and a zero-success page whose cursor permits a complete retry
without skipping the first item. The sentinel matrix checks returned values,
provider fakes, control state, cursors, identifiers, addresses, message
content, titles, encrypted bytes, checksums, and encryption metadata. The
trait-object test constructs every provider future and asserts `Send`; every
fake type is asserted `Send + Sync`.

## RED

No historical failing-first transcript was preserved in this checkout. The
tests and fakes were assembled together, so this report does not invent a
pre-implementation failure. Static review reconstructed the actionable
omission: the first matrix did not exercise `CalendarOwnerFind` and
`CalendarProposalFind`. The current matrix now seeds each lookup and runs the
same closed-error, exact-count, no-mutation, and recovery contract for both.

## GREEN — LOCAL

All commands below were run locally in `/private/tmp/agent-voice-pa-task5-build`
against the current Task 5 provider/fake sources.

```text
rtk cargo test --lib pa::fakes::contract_tests -- --nocapture
cargo test: 8 passed, 428 filtered out (1 suite, 0.00s)

rtk cargo test --lib pa::fakes -- --nocapture
cargo test: 109 passed, 327 filtered out (1 suite, 0.01s)

rtk cargo test --lib
cargo test: 436 passed (2 suites, 7.30s)

rtk cargo test --doc
cargo test: 3 passed (1 suite, 2.16s)

RUSTDOCFLAGS='-D warnings' rtk cargo doc --no-deps
Documented agent_voice v0.1.0; finished successfully and generated target/doc/agent_voice/index.html.

rtk rustfmt --edition 2024 --check src/pa/providers.rs src/pa/fakes/backup.rs src/pa/fakes/calendar.rs src/pa/fakes/contract_tests.rs src/pa/fakes/control.rs src/pa/fakes/mail.rs src/pa/fakes/mod.rs src/pa/fakes/triage.rs
passed (no output)

rtk git diff --check
passed (no output)
```

The focused selector runs all eight matrix tests, including the happy-path,
sentinel redaction, common-failure, cursor, clone/concurrency, and `Send`
checks. The full library and doctest runs are local evidence only.

## Review and remediation

- Finding: the initial cross-provider matrix did not cover owner-event and
  Google-proposal lookup failures.
- Remediation: `every_operation_reports_common_failures_with_exact_counts_and_recovers`
  now invokes `CalendarOwnerFind` and `CalendarProposalFind` through their
  public trait objects, with all three closed failures, exact counts, and
  recovery assertions. The trait-object `Send` matrix also includes both
  lookup futures.
- Evidence: the focused matrix and full local library suite pass after the
  additions. No unresolved package-local finding is claimed beyond the
  external CI review still required for the implementation PR.

## Non-claims

- **CI:** not run or inferred here. Remote workflow status must be recorded on
  the implementation PR after it is opened.
- **LIVE:** no Microsoft Graph, Gmail, Google Calendar, S3, OAuth, network, or
  production-provider operation was performed. Fake tests do not establish
  live-provider readiness.
- **Production behavior:** this package proves public fake/provider contracts;
  it does not prove real provider payload compatibility, credentials, rollout,
  deployment, backup restore, or operational alerting.

## PR linkage

The implementation PR is not opened by this package. Its body must include
`Closes #200` and must link the Task 5 parent/preceding package work as
appropriate. This report contains no commit, push, GitHub mutation, OAuth
consent, or deployment evidence.
