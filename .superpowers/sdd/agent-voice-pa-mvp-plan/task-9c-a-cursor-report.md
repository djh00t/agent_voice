# Task 9c-A report: durable provider cursor CAS

- **Issue:** [#290](https://github.com/djh00t/agent_voice/issues/290)
- **Package:** `task-9c-a`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Base revision:** `a20a28b` (`origin/main` after #305/#306/#307/#309)
- **Implementation revision:** `498eff8`
- **Readback timestamp:** `2026-08-31T14:00:10Z`

## Contract and prerequisite readback

PR #204 is merged as `b42498e`; it delivers the encrypted store package and
the prior cursor repository package tracked by #136 and #142. PR #205 is
merged as the same chain and delivers the typed provider-neutral contracts
tracked by #157. The merge commit was verified as an ancestor of
`origin/main`. Issues #136, #142, and #157 are closed. Parent #88 remains
open/status:blocked and owns coordination only. PR #306 is merged in
`ebb4a8c`; the final integration rebase onto `a20a28b` completed without
conflicts. The #306 store changes remain in the rebased tree, while the final
diff from `origin/main` is limited to this package's cursor symbols/tests.

Issue #290's binding clarification ([comment
5479034348](https://github.com/djh00t/agent_voice/issues/290#issuecomment-5479034348))
resolves the wording conflict between the frozen no-whitespace shorthand and
the required #157 provider boundary. Cursor values use the provider-compatible
contract: nonblank bounded ASCII with control characters rejected, including
printable punctuation and embedded spaces. Stream IDs remain machine-safe and
reject whitespace. Neither side trims, normalizes, decodes, or reserializes
values.

The implementation uses the existing encrypted `provider_cursors` table and
existing redacted `StoreError` categories. It adds no migration, schema,
provider, worker, OAuth, network, or dependency behavior.

## Owned paths and atomic commits

This package owns the cursor methods, private cursor helpers, and cursor-focused
inline tests in `src/pa/store.rs`, plus this report. No external cursor test
file was required. No other source, migration, schema, provider, worker,
configuration, dependency, or report path changed.

The implementation history is one-file and one-logical-change per commit:

- `5fe68f6` — test-only cursor contract and exact selectors (`src/pa/store.rs`)
- `c3bbf76` — cursor API and immediate CAS implementation (`src/pa/store.rs`)
- `bdefbd7` — initial evidence report (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `5f468fc` — post-rebase evidence refresh (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `1b2f76d` — provider-compatible cursor alphabet regression (`src/pa/store.rs`)
- `52e0831` — provider-compatible cursor validation (`src/pa/store.rs`)
- `f0765a7` — stream machine-safe regression (`src/pa/store.rs`)
- `498eff8` — separate stream and cursor validation (`src/pa/store.rs`)
- `a351eb6` — cursor review repair evidence (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `91bb984` — evidence refresh after integrating current main (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `91d4cbe` — final post-rebase selector and gate evidence (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `2cd5f94` — remove the obsolete post-#306 rollback instruction (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `3f88539` — finalize cursor handoff evidence (`.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)
- `HEAD` — this final report-only commit (self-reference intentionally has no literal SHA; `.superpowers/sdd/agent-voice-pa-mvp-plan/task-9c-a-cursor-report.md`)

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

During review repair, the exact invalid-input selector was also run after the
stream-whitespace regression test and before the separate stream validator
existed. It exited nonzero because the provider-compatible cursor validator
was still being applied to stream IDs. This was a real named-test failure, not
a filtered zero-test result.

```text
rtk cargo test --lib pa::store::tests::provider_cursor_invalid_inputs_are_atomic_and_redacted -- --exact --nocapture
exit 101
test result: FAILED; 0 passed; 1 failed; 0 ignored; 0 measured; 550 filtered out
```

## GREEN

The five required exact selectors each matched one test and passed:

```text
rtk cargo test --lib pa::store::tests::provider_cursor_cas_rejects_stale_and_equal_retry -- --exact --nocapture
exit 0
cargo test: 1 passed, 569 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_first_write_and_restart -- --exact --nocapture
exit 0
cargo test: 1 passed, 569 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_two_handles_have_one_winner -- --exact --nocapture
exit 0
cargo test: 1 passed, 569 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_invalid_inputs_are_atomic_and_redacted -- --exact --nocapture
exit 0
cargo test: 1 passed, 569 filtered out (1 suite)

rtk cargo test --lib pa::store::tests::provider_cursor_streams_are_isolated -- --exact --nocapture
exit 0
cargo test: 1 passed, 569 filtered out (1 suite)
```

The aggregate cursor selector matched all five cursor tests and the full store
module remained green:

```text
rtk cargo test --lib pa::store::tests::provider_cursor -- --nocapture
exit 0
cargo test: 5 passed, 565 filtered out (1 suite)

rtk cargo test --lib pa::store -- --nocapture
exit 0
cargo test: 181 passed, 389 filtered out (1 suite)
```

The tests cover absent and nullable rows as `None`, first insert, encrypted
file close/reopen, equal retry with stable timestamp and row count, exact
stale/out-of-order conflicts, duplicate first-write rejection, stream
isolation, two-handle one-winner behavior, provider-compatible printable
cursor values, machine-safe stream values, invalid input before mutation, and
redacted invalid/corrupt-state errors. They do not print fixture values.

## Checks

| Command | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 --check src/pa/store.rs` | PASS, exit 0 |
| `rtk git diff --check -- src/pa/store.rs` | PASS, exit 0 |
| `rtk make docs-install` | PASS, exit 0; checked-in website lockfile used |
| `rtk make check` | PASS, exit 0; 570 Rust tests, Clippy, rustdoc, and Docusaurus completed after the final validation fix |

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

- `{tier: LOCAL, kind: RED, selector_or_scope: provider_cursor_cas_rejects_stale_and_equal_retry, command_or_check: exact cargo test selector, expected: true named match and nonzero missing-API failure, exit_code: 101, observed_redacted: 37 compile errors for absent API and old return shape, source_revision: 3f0bf1e, timestamp_utc: 2026-08-31T13:12:17Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: five exact provider cursor selectors, command_or_check: exact cargo test selectors, expected: one matched test per selector, exit_code: 0, observed_redacted: 1 passed and 569 filtered out for each, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: provider_cursor aggregate, command_or_check: aggregate cargo test selector, expected: all cursor tests match and pass, exit_code: 0, observed_redacted: 5 passed and 565 filtered out, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: pa::store module, command_or_check: full store cargo test selector, expected: no store regressions, exit_code: 0, observed_redacted: 181 passed and 389 filtered out, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: src/pa/store.rs formatting, command_or_check: scoped rustfmt check, expected: no formatting diff, exit_code: 0, observed_redacted: clean, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: owned diff whitespace, command_or_check: scoped git diff check, expected: no whitespace errors, exit_code: 0, observed_redacted: clean, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: LOCAL, kind: GREEN, selector_or_scope: repository changed-scope gate, command_or_check: rtk make check after locked docs setup, expected: Rust tests, Clippy, rustdoc, and Docusaurus pass, exit_code: 0, observed_redacted: complete gate passed after final validation fix and main integration rebase, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: LOCAL, kind: REVIEW, selector_or_scope: whole-tree formatting, command_or_check: rtk cargo fmt --all -- --check, expected: 0 differences, exit_code: 1, observed_redacted: executed check found pre-existing drift only in out-of-scope files; owned file remains clean, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: STATIC, kind: REVIEW, selector_or_scope: prerequisite and ownership readback, command_or_check: issue/PR state readback, merge-ancestor check, and origin-main diff review, expected: merged gates and only owned cursor paths, exit_code: 0, observed_redacted: #136/#142/#157 merged and closed; #88 blocked coordination parent; #306 merged; source diff limited to cursor source/tests after final rebase, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: STATIC, kind: REVIEW, selector_or_scope: validation clarification, command_or_check: #290 binding clarification and #157 provider validator readback, expected: distinct stream/cursor alphabets, exit_code: 0, observed_redacted: cursor provider-compatible printable ASCII; stream machine-safe; no normalization, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: STATIC, kind: REVIEW, selector_or_scope: CAS and redaction boundary, command_or_check: source symbol and diff scan, expected: one immediate parameterized CAS and fixed errors, exit_code: 0, observed_redacted: no legacy save/delete cursor API, no migration, and no raw fixture value in test output, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: STATIC, kind: REVIEW, selector_or_scope: atomic commit history, command_or_check: git show --name-status for every delivery commit, expected: one file per commit, exit_code: 0, observed_redacted: each source/test commit touches src/pa/store.rs only and each report commit touches the report only, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`
- `{tier: CI, kind: GREEN, selector_or_scope: linked workflow, command_or_check: GitHub check readback for PR #311, expected: all required checks pass, exit_code: 0, observed_redacted: 5 checks passed and 0 failed (Quality Gates, Compose Config, CodeQL JavaScript/TypeScript, CodeQL Rust, and CodeQL aggregate), source_revision: 2cd5f94, timestamp_utc: 2026-08-31T13:56:46Z}`
- `{tier: LIVE, kind: NOT_RUN, selector_or_scope: provider and deployment boundary, command_or_check: live-provider/OAuth/network/deployment check, expected: not applicable to this local slice, exit_code: NOT_RUN, observed_redacted: no live operation or credential use, source_revision: 498eff8, timestamp_utc: 2026-08-31T13:38:00Z}`

## Lifecycle, review, and rollback

Issue #290 was moved from OPEN/status:blocked to OPEN/status:in-progress only
after the merged prerequisite readback, then to OPEN/status:review after PR
#311 and this report were read back. It remains open; this package does not
close #290 or any parent/prerequisite issue.

Review repair added provider-compatible cursor coverage and separate
machine-safe stream validation in `1b2f76d`/`52e0831`/`f0765a7`/`498eff8`,
with the exact selectors and full local gate rerun above. The atomicity finding
was checked against the exhaustive per-commit file list, and the malformed
report message was reworded during the interactive rebase. Fresh review of
`91d4cbe` found one stale rollback sentence about #306; docs-only repairs
`2cd5f94` and `3f88539` removed that sentence and completed the exact delivery
history/readback. Fresh review of `3f88539` also corrected the final 570-test
gate count and classified the executed whole-tree formatter residual as a
review finding rather than an unexecuted check; `HEAD` records this repair.
The final pushed head has five green CI checks; a fresh independent review
remains the handoff verification step. No unresolved implementation finding is
claimed here.

Rollback is a reviewed reverse-order revert of the report, implementation, and
test commits. It does not remove the existing encrypted schema or durable data,
alter provider contracts, or change the coordination parent. PR #306 is already
merged in the final base; no additional prerequisite rebase remains for this
package.
