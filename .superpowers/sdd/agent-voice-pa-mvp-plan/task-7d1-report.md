# Task 7d.1a verification report

## Ownership and prerequisites

- Issue: #256; parent: #68; feature: #58.
- Owned implementation: `src/pa/store.rs` only.
- Owned evidence: this report only.
- Required predecessor: #255 v13 configuration-revision migration, merged before delivery. Downstream typed values/decoding are #269; reserve/takeover and complete/release remain later packages.
- Excluded: HTTP handlers/middleware, public idempotency values, fingerprints, decoding, state transitions, race behavior, dependencies, providers, deployment, OAuth, cluster state, and UAT.

## Revision and dirty tree

- Base revision supplied by the controller: `78f03dd2b33be2a276a5b00b80026ca0dc687a49`.
- Earlier implementation commit: `446a66f8dea4b6adcbfeefcff8b4b9c69853491d` (`feat(pa-store): add durable HTTP idempotency schema`).
- Round-1 source commit: `25a905225f34641699d5fa08fc3da3110ca6e2c6` (`test(pa-store): strengthen HTTP idempotency schema proof`).
- Round-2 source follow-up commit: `25074b38e5a0e0bfe9c2505bb669badddbc740e7` (`test(pa-store): assert reopened idempotency row content`).
- Before this round, tracked files were clean; the pre-existing untracked reviewer directory `.superpowers/sdd/issue-256/` was preserved and not touched. After the source commit, only this report was changed by this task; no excluded file changed.
- Tool snapshot: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, `cargo 1.97.1 (c980f4866 2026-06-30)`.
- Evidence timestamp: `2026-08-31T04:12:16+1000` command-session snapshot; round-2 checks ran against source commit `25074b38e5a0e0bfe9c2505bb669badddbc740e7`.

## RED provenance

The required test was added before the v14 migration body. The exact RED invocation was:

```text
rtk cargo test --lib pa::store::tests::http_idempotency_v14_migration_creates_schema -- --exact
```

Observed exit `101`; assertion observed schema version `13` and expected `14`; `0 passed; 1 failed; 510 filtered out`. This was the expected missing-schema failure, not a filtered zero-test pass.

## Focused selectors and GREEN evidence

Each selector below ran with `--exact` and passed one test (`513 filtered out`):

```text
rtk cargo test --lib pa::store::tests::http_idempotency_v14_migration_creates_schema -- --exact
rtk cargo test --lib pa::store::tests::http_idempotency_v14_migration_is_idempotent -- --exact
rtk cargo test --lib pa::store::tests::http_idempotency_v14_constraints_reject_invalid_rows -- --exact
rtk cargo test --lib pa::store::tests::http_idempotency_v14_reopen_preserves_rows -- --exact
```

The focused runs all exited `0`. Scoped checks also exited `0`:

```text
rtk rustfmt --edition 2024 --check src/pa/store.rs
rtk git diff --check -- src/pa/store.rs
```

Round-1 test-first note: the structural assertions were added before the
review correction was committed and the exact structural selector passed
immediately because the existing v14 migration already had the required
shape. No production correction was needed for that finding; the prior true
nonzero RED remains recorded above.

## Fresh full rerun

`rtk make check` exited `0` on the implementation revision. The run compiled the test and documentation profiles, ran the full Rust test suite (`514` tests observed), Clippy with warnings denied, rustdoc, and the locked website build. No warnings or failures remained in the command result.

## Round-2 fresh verification

After the round-2 source follow-up commit, all four exact selectors exited `0`, each with `1 passed; 513 filtered out`. `rtk rustfmt --edition 2024 --check src/pa/store.rs` exited `0`; unscoped `rtk git diff --check` exited `0`; and a fresh `rtk make check` exited `0`, including the observed `514`-test suite, Clippy, rustdoc, and locked website checks.

## Round-3 pull-request review remediation

Codex review on PR #273 correctly identified that `NOT NULL` alone accepted
blank durable identities. The focused constraint selector first exited `101`
with `empty scope was accepted`. Source-only commit `7a874473ae32d179d9f5068dfafe81afe7bb91c9`
adds trimmed non-blank checks for scope, idempotency key, and fingerprint and
covers empty and whitespace-only values for all three fields. All four exact
schema selectors then exited `0`; a fresh controller `rtk make check` exited
`0` with 514 tests, three compile-fail doctests, Clippy, rustdoc, and the
locked website build. The separate cumulative-diff commit-splitting finding
was answered with GitHub metadata proving every PR commit changes one file.

## Round-4 SQLite storage hardening

Current-head review found that one-argument SQLite `trim()` removes only ASCII
space and that column affinity alone does not enforce storage classes. RED
coverage proved tab-only identities and wrong-type fields were accepted.
Source-only commit `6d5657468cd5168bd35e85b02c7c7712b4b68faa` now enforces the exact bounded ASCII identifier
grammars at the schema boundary and requires the contracted SQLite storage
class for every required field and each present response field. All five v14
selectors passed after the fix. A fresh controller `rtk make check` exited `0`
with 515 tests, three compile-fail doctests, Clippy, rustdoc, and the locked
website build.

## Round-5 canonical timestamp remediation

Current-head review found that SQLite `CURRENT_TIMESTAMP` emits a legacy
space-separated value that the store's strict RFC3339 UTC decoder rejects.
RED coverage failed on the old default metadata and proved a legacy stored
timestamp was accepted. Source-only commit
`7ad0976e9868a5b2e8da5ed790d2d37e07d4b690` now uses the established
`strftime('%Y-%m-%dT%H:%M:%SZ', 'now')` defaults and exact canonical checks for
both timestamp columns. All six v14 selectors passed after the fix; the
implementer gate observed 516 tests plus lint, rustdoc, and docs stages.

## Round-6 embedded-NUL remediation

Current-head review proved SQLite `GLOB` could stop at an embedded NUL while
the byte-length constraint still counted an invalid suffix. RED failed with
`embedded NUL in scope was accepted`. Source-only commit
`c64dd7f2c3f77c18155b428fd66670965776eb06` adds explicit BLOB NUL rejection
for scope, idempotency key, and fingerprint, including an exact-64-byte
fingerprint bypass fixture. All exact v14 selectors and the implementer full
gate passed with 516 tests.

## Round-7 temporal grammar remediation

Current-head review proved arbitrary `lease_until` text and hour-24 RFC3339
values could enter the durable table. RED failed with `empty lease_until was
accepted` and `hour 24 created_at was accepted`. Source-only commit
`50f88723c06886b35610cc865d56249dda619191` enforces canonical unsigned
decimal Unix-second lease text and explicit UTC timestamp component ranges.
All seven exact v14 selectors and the implementer full gate passed with 517
tests and three compile-fail doctests.

## Evidence records

| evidence_id | order | command | expected_exit | observed_exit | selector_or_diagnostic | evidence_tier | status | artifact_ref | non_claim | reviewer |
|---|---:|---|---:|---:|---|---|---|---|---|---|
| EV-13F-RED-01 | 0 | `rtk cargo test --lib pa::store::tests::http_idempotency_v14_migration_creates_schema -- --exact` | nonzero | 101 | missing v14 table; observed 13 vs 14 | LOCAL | CONFIRMED | `NONE` | proves only the pre-implementation RED test | NONE |
| EV-13F-01 | 1 | `rtk cargo test --lib pa::store::tests::http_idempotency_v14_migration_creates_schema -- --exact` | 0 | 0 | v14 table, unique key, state contract, lease index | LOCAL | CONFIRMED | `NONE` | does not prove typed values or HTTP behavior | NONE |
| EV-13F-02 | 2 | `rtk cargo test --lib pa::store::tests::http_idempotency_v14_migration_is_idempotent -- --exact` | 0 | 0 | one v14 migration/table/index after reapply | LOCAL | CONFIRMED | `NONE` | does not prove concurrent reservation behavior | NONE |
| EV-13F-03 | 3 | `rtk cargo test --lib pa::store::tests::http_idempotency_v14_constraints_reject_invalid_rows -- --exact` | 0 | 0 | invalid state/generation/required/duplicate/response rows rejected | LOCAL | CONFIRMED | `NONE` | does not prove downstream decoding or transitions | NONE |
| EV-13F-04 | 4 | `rtk cargo test --lib pa::store::tests::http_idempotency_v14_reopen_preserves_rows -- --exact` | 0 | 0 | v13 data and exact (`scope`, `key`, `fingerprint`) row content survive three reopens with stable counts | LOCAL | CONFIRMED | `NONE` | does not prove deployment or recovery UAT | NONE |
| EV-13F-05 | 5 | `rtk rustfmt --edition 2024 --check src/pa/store.rs` | 0 | 0 | owned source formatting | LOCAL | CONFIRMED | `NONE` | does not prove CI | NONE |
| EV-13F-06 | 6 | `rtk git diff --check -- src/pa/store.rs` | 0 | 0 | owned source whitespace | STATIC | CONFIRMED | `NONE` | does not prove semantic review by another person | NONE |
| EV-13F-07 | 7 | `rtk git diff --check` | 0 | 0 | unscoped repository whitespace | STATIC | CONFIRMED | `NONE` | does not prove excluded untracked artifacts are safe | NONE |
| EV-13F-08 | 8 | `rtk make check` | 0 | 0 | 514-test suite, Clippy, rustdoc, docs | LOCAL | CONFIRMED | `NONE` | does not prove CI, live, provider, OAuth, cluster, or UAT behavior | NONE |
| EV-13F-09 | 9 | `rtk cargo test --lib pa::store::tests::http_idempotency_v14_constraints_reject_invalid_rows -- --exact` | nonzero then 0 | 101 then 0 | empty and whitespace-only scope/key/fingerprint rejected | LOCAL | CONFIRMED | `7a874473ae32d179d9f5068dfafe81afe7bb91c9` | does not prove runtime decoding or reservation behavior | Codex PR review |
| EV-13F-10 | 10 | `rtk make check` | 0 | 0 | post-review 514-test suite, compile-fail doctests, Clippy, rustdoc, docs | LOCAL | CONFIRMED | `7a874473ae32d179d9f5068dfafe81afe7bb91c9` | does not prove current-head CI or live behavior | controller |
| EV-13F-11 | 11 | v14 invalid-row and wrong-storage-class exact selectors | nonzero then 0 | 101 then 0 | non-space whitespace and wrong SQLite storage classes rejected | LOCAL | CONFIRMED | `6d5657468cd5168bd35e85b02c7c7712b4b68faa` | does not prove runtime decoding | Codex PR review |
| EV-13F-12 | 12 | `rtk make check` | 0 | 0 | post-hardening 515-test suite, compile-fail doctests, Clippy, rustdoc, docs | LOCAL | CONFIRMED | `6d5657468cd5168bd35e85b02c7c7712b4b68faa` | does not prove current-head CI or live behavior | controller |
| EV-13F-13 | 13 | v14 migration metadata and timestamp exact selectors | nonzero then 0 | 101 then 0 | canonical RFC3339 UTC defaults and malformed timestamp rejection | LOCAL | CONFIRMED | `7ad0976e9868a5b2e8da5ed790d2d37e07d4b690` | does not prove runtime reservation behavior | Codex PR review |
| EV-13F-14 | 14 | v14 invalid-row exact selector | nonzero then 0 | 101 then 0 | embedded-NUL suffix bypasses rejected for all durable identities | LOCAL | CONFIRMED | `c64dd7f2c3f77c18155b428fd66670965776eb06` | does not prove runtime decoding | Codex PR review |
| EV-13F-15 | 15 | v14 lease and timestamp exact selectors | nonzero then 0 | 101 then 0 | canonical decimal leases and RFC3339 component bounds enforced | LOCAL | CONFIRMED | `50f88723c06886b35610cc865d56249dda619191` | does not prove runtime reservation behavior | Codex PR review |

## Round-1 findings and resolutions

- Structural schema proof: resolved by exact `PRAGMA table_info` assertions
  for all twelve v14 columns, including type, not-null, default, and primary
  key metadata; exact lease-index `PRAGMA index_list` metadata and columns are
  also asserted.
- Partial response constraints: resolved by distinct `response_status`-only,
  `response_content_type`-only, and `response_body`-only in-progress inserts,
  each required to fail without changing the two valid rows.
- Reopen/preservation proof: resolved by seeding a v13 configuration row and
  replay nonce, then opening the file three times while asserting both rows,
  the exact idempotency `(scope, key, fingerprint)` tuple, and stable
  table/index/configuration/replay/idempotency/migration counts.
- Unscoped diff evidence: resolved by running `rtk git diff --check` with exit
  `0`; the pre-existing untracked reviewer directory remains outside the
  owned diff and was not staged.

## Schema and scope review

- `CURRENT_SCHEMA_VERSION` is 14 and v13 remains an explicit preceding migration.
- v14 creates `http_idempotency_records` with required, trimmed non-blank
  scope/key/fingerprint fields, state/generation/lease fields, the unique
  `(scope, idempotency_key)` invariant, explicit in-progress/completed response
  nullability/status/content-type checks, and
  `idx_http_idempotency_records_lease_until`.
- Migration execution remains transactional through the existing migration runner; `schema_migrations` records version 14 once.
- Source diff is limited to v14 registration/body, four v14 schema tests with
  strengthened structural/partial/reopen assertions, and the two existing
  schema inventory/version assertions required by the new version.

## Failure, security, and redaction review

Malformed schema assumptions, failed constraints, duplicate migration/index, unexpected version, nonzero named test, or changed prior data blocks delivery. Test data is synthetic. No secrets, credentials, signatures, request bodies, provider identifiers, transcripts, or environment dumps are emitted by the report or tests. No provider, OAuth, SIP, deployment, cluster, or paid action was run.

## Tiered evidence and residual gates

- `LOCAL`: RED, focused tests, rustfmt, diff check, and `rtk make check` above.
- `STATIC`: source diff and schema/scope review; independent reviewer still required.
- `CI`: the pre-remediation PR head passed five checks; current-head CI and
  re-review were triggered after `7a874473` and remain required before handoff.
- `LIVE`: `UNEXECUTED`; no deployment, provider, OAuth, cluster, or UAT evidence exists or is claimed.

Residual gates: #255/PR #265 must be merged and verified before delivery;
current-head CI and re-review must pass; rebase/retarget and current-base rerun
remain required after the prerequisite merges; #269 and later transition
packages remain separate. PR #273 is pushed, open, and stacked on PR #265.

## Reviewer and lifecycle

Controller review must verify the exact base/stack, one-file source commit, migration transaction/idempotency, constraint semantics, no downstream leakage, and all evidence rows. If #265/#255 changes, stop and rebase/retarget before further work. The future delivering PR must contain `Closes #256`, `Refs #255`, `Refs #68`, and `Refs #58`; it must not promote LOCAL/STATIC results to CI/LIVE.

## Completion evidence

- Implementer: isolated stack owner; exact identity not recorded.
- Source commit: `25a905225f34641699d5fa08fc3da3110ca6e2c6`.
- Round-2 source follow-up: `25074b38e5a0e0bfe9c2505bb669badddbc740e7`.
- Round-3 source remediation: `7a874473ae32d179d9f5068dfafe81afe7bb91c9`.
- Round-4 source remediation: `6d5657468cd5168bd35e85b02c7c7712b4b68faa`.
- Round-5 source remediation: `7ad0976e9868a5b2e8da5ed790d2d37e07d4b690`.
- Round-6 source remediation: `c64dd7f2c3f77c18155b428fd66670965776eb06`.
- Round-7 source remediation: `50f88723c06886b35610cc865d56249dda619191`.
- Prior report commits: `0f711bf03eecf009980de96e8b4990fb5de9389a`, `ddc35eab9aff4955d11b8ac1d54d5557c4bbaa8f`.
- Report commit state: containing commit; resolve with `rtk git log -1 --format=%H -- .superpowers/sdd/agent-voice-pa-mvp-plan/task-7d1-report.md` after this report is committed.
- Reviewer: pending controller review.
- CI/LIVE/provider/OAuth/cluster/UAT: `UNEXECUTED`.
