# Task 9c-A report: durable provider cursor CAS

- **Issue:** [#290](https://github.com/djh00t/agent_voice/issues/290)
- **Package:** `task-9c-a`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Base revision:** `f8319ac` (`origin/main` at worktree creation)
- **Implementation revision:** `2e2584e`
- **Readback timestamp:** `2026-08-31T13:08:43Z`

## Contract and prerequisite readback

PR #204 is merged as `b42498e`; it delivers the encrypted store package and
the prior cursor repository package tracked by #136 and #142. PR #205 is
merged as the same chain and delivers the typed provider-neutral contracts
tracked by #157. The merge commit was verified as an ancestor of
`origin/main`. Issues #136, #142, and #157 are closed. Parent #88 remains
open/status:blocked and owns coordination only. PR #306 remains open, so this
package is intentionally based on the current base readback before that PR
and must be rebased after #306 merges.

The implementation uses the existing encrypted `provider_cursors` table and
existing redacted `StoreError` categories. It adds no migration, schema,
provider, worker, OAuth, network, or dependency behavior.

## Owned paths and atomic commits

This package owns the cursor methods, private cursor helpers, and cursor-focused
inline tests in `src/pa/store.rs`, plus this report. No external cursor test
file was required. No other source, migration, schema, provider, worker,
configuration, dependency, or report path changed.

The implementation history is one-file and one-logical-change per commit:

- `c05cc2f` — test-only cursor contract and exact selectors (`src/pa/store.rs`)
- `2e2584e` — cursor API, validation, and immediate CAS implementation (`src/pa/store.rs`)
- report commit — this evidence file only

The final changed-path readback after the report commit must contain only
`src/pa/store.rs` and this report relative to the original base.

## RED

The required named selector was run after the tests were added and before the
new production API existed. It exited nonzero because the API was absent and
the old load return type did not satisfy the new `Option` assertions. The
filter matched a real test; no zero-test result was counted.

```text
rtk cargo test --lib pa::store::tests::provider_cursor_cas_rejects_stale_and_equal_retry -- --exact --nocapture
exit 101
cargo test: 37 errors, 0 warnings (1 suite)
```

## GREEN

The five required exact selectors each matched one test and passed:

```text
rtk cargo test --lib pa::store::tests::provider_cursor_cas_rejects_stale_and_equal_retry -- --exact --nocapture
exit 0
cargo test: 1 passed, 550 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_first_write_and_restart -- --exact --nocapture
exit 0
cargo test: 1 passed, 550 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_two_handles_have_one_winner -- --exact --nocapture
exit 0
cargo test: 1 passed, 550 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_invalid_inputs_are_atomic_and_redacted -- --exact --nocapture
exit 0
cargo test: 1 passed, 550 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_streams_are_isolated -- --exact --nocapture
exit 0
cargo test: 1 passed, 550 filtered out (1 suite)
```

The aggregate cursor selector matched all five cursor tests and the full store
module remained green:

```text
rtk cargo test --lib pa::store::tests::provider_cursor -- --nocapture
exit 0
cargo test: 5 passed, 546 filtered out (1 suite)

rtk cargo test --lib pa::store -- --nocapture
exit 0
cargo test: 178 passed, 373 filtered out (1 suite)
```

The tests cover absent and nullable rows as `None`, first insert, encrypted
file close/reopen, equal retry with stable timestamp and row count, exact
stale/out-of-order conflicts, duplicate first-write rejection, stream
isolation, two-handle one-winner behavior, invalid input before mutation, and
redacted invalid/corrupt-state errors. They do not print fixture values.

## Checks

| Command | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 --check src/pa/store.rs` | PASS, exit 0 |
| `rtk git diff --check -- src/pa/store.rs` | PASS, exit 0 |
| `rtk make docs-install` | PASS, exit 0; checked-in website lockfile used |
| `rtk make check` | PASS, exit 0; 551 Rust tests, Clippy, rustdoc, and Docusaurus completed |

The first `rtk make check` attempt exited 2 at `docs-build` because the fresh
worktree did not yet have the checked-in website dependencies installed. No
source or dependency manifest changed; the locked `docs-install` setup above
was run and the complete gate was rerun successfully. npm reported 24 audit
advisories (7 moderate, 17 high) during setup; no remediation was performed.

The required whole-tree formatter command was also run:

```text
rtk cargo fmt --all -- --check
exit 1
```

Its only reported differences were pre-existing formatting drift in the
out-of-scope `src/pa/fakes/mail.rs` and `src/service.rs`. The owned
`src/pa/store.rs` scoped formatter check passed, and no out-of-scope file was
changed.

## Evidence records

- `{tier: LOCAL, kind: RED, selector_or_scope: provider_cursor_cas_rejects_stale_and_equal_retry, command_or_check: exact cargo test selector, expected: true named match and nonzero missing-API failure, exit_code: 101, observed_redacted: 37 compile errors for absent API and old return shape, source_revision: c05cc2f, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: five exact provider cursor selectors, command_or_check: exact cargo test selectors, expected: one matched test per selector, exit_code: 0, observed_redacted: 1 passed and 550 filtered out for each, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: provider_cursor aggregate, command_or_check: aggregate cargo test selector, expected: all cursor tests match and pass, exit_code: 0, observed_redacted: 5 passed and 546 filtered out, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: pa::store module, command_or_check: full store cargo test selector, expected: no store regressions, exit_code: 0, observed_redacted: 178 passed and 373 filtered out, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: src/pa/store.rs formatting, command_or_check: scoped rustfmt check, expected: no formatting diff, exit_code: 0, observed_redacted: clean, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: owned diff whitespace, command_or_check: scoped git diff check, expected: no whitespace errors, exit_code: 0, observed_redacted: clean, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: repository changed-scope gate, command_or_check: rtk make check after locked docs setup, expected: Rust tests, Clippy, rustdoc, and Docusaurus pass, exit_code: 0, observed_redacted: complete gate passed, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LOCAL, kind: NOT_RUN, selector_or_scope: whole-tree formatting, command_or_check: rtk cargo fmt --all -- --check, expected: exit 0, exit_code: 1, observed_redacted: pre-existing drift only in out-of-scope files, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: STATIC, kind: REVIEW, selector_or_scope: prerequisite and ownership readback, command_or_check: issue/PR state readback, merge-ancestor check, and origin-main diff review, expected: merged gates and only owned cursor paths, exit_code: 0, observed_redacted: #136/#142/#157 merged and closed; #88 blocked coordination parent; #306 open; source diff limited to cursor source/tests before report, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: STATIC, kind: REVIEW, selector_or_scope: CAS and redaction boundary, command_or_check: source symbol and diff scan, expected: one immediate parameterized CAS and fixed errors, exit_code: 0, observed_redacted: no legacy save/delete cursor API, no migration, and no raw fixture value in test output, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: CI, kind: NOT_RUN, selector_or_scope: linked workflow, command_or_check: GitHub check readback, expected: independently observed result, exit_code: UNEXECUTED, observed_redacted: no CI result claimed, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`
- `{tier: LIVE, kind: NOT_RUN, selector_or_scope: provider and deployment boundary, command_or_check: live-provider/OAuth/network/deployment check, expected: not applicable to this local slice, exit_code: NOT_RUN, observed_redacted: no live operation or credential use, source_revision: 2e2584e, timestamp_utc: 2026-08-31T13:08:43Z}`

## Lifecycle, review, and rollback

Issue #290 was moved from OPEN/status:blocked to OPEN/status:in-progress only
after the merged prerequisite readback. It may move to status:review only after
the delivering PR and this report are read back. It remains open; this package
does not close #290 or any parent/prerequisite issue.

No independent reviewer result is available at report creation. Review remains
a PR gate; unresolved findings block status:review and closure. CI and LIVE
remain unexecuted/not applicable as recorded above.

Rollback is a reviewed reverse-order revert of the report, implementation, and
test commits. It does not remove the existing encrypted schema or durable data,
alter provider contracts, or change the coordination parent. Because PR #306
is still open, rebase this branch and refresh the source/revision evidence after
#306 merges before handoff for merge.
