# Task 11h.0a — durable backup-attempt schema v15

## Scope and provenance

- Issue: #266
- Parent: #112
- Feature tracker: #62
- Merged prerequisite: #256 / PR #273
- Base: `683249e59f8b302bbf8dc7a1ba9a1f0ab1d076d4`
- Source head: `03a3256fac72050d4e5f69b9dafcdaa22f25a875`
- Runtime owner: `src/pa/store.rs`
- Contract correction: https://github.com/djh00t/agent_voice/issues/266#issuecomment-5486352687

The package adds schema only. It exposes no backup-attempt API, performs no backup/provider operation, and changes no route, configuration, dependency, or deployment artifact.

## Schema contract

Migration 15 creates `backup_operation_attempts` with a unique attempt key, four allowed operation values, three lifecycle states, terminal-field consistency, canonical UTC defaults, and `idx_backup_operation_attempts_operation_started` ordered by operation then descending start time and id.

Independent review identified that the originally frozen bare `CURRENT_TIMESTAMP` default conflicts with the store's canonical RFC3339 UTC contract. The issue addendum superseded only those two defaults with `strftime('%Y-%m-%dT%H:%M:%SZ', 'now')`; the package still delegates lifecycle timestamp validation to its downstream API issue.

## Acceptance evidence

| Requirement | Evidence |
| --- | --- |
| True TDD RED | The exact migration selector failed with schema version 14 before implementation. The canonical-default selector later failed on the original 19-byte SQLite timestamp before the corrected default. |
| Fresh v15 schema | `migration_v15_adds_backup_attempt_schema` passed with exact columns, defaults, table version, and index. |
| Canonical generated timestamps | An insert omitting both timestamp columns produced parseable 20-byte whole-second UTC values that round-trip to the canonical format. |
| Index direction | `PRAGMA index_xinfo` proved operation ascending, then `started_at` and `id` descending. |
| Preservation and idempotency | All existing v14 table row counts and seeded representative values matched before migration, after migration, and after a second reopen; migration 15 was recorded once. |
| Constraint boundary | Valid operation/state shapes succeeded and invalid operation, state, and terminal combinations were rejected. |
| Focused suite | All four issue selectors passed; full `pa::store::tests` passed with 186 tests. |
| Repository gate | Fresh `rtk make check` passed after the complete source change: 584 tests. |
| Static checks | Owned-file rustfmt and `rtk git diff --check` passed. |

## Independent review

The first review found the timestamp-default mismatch, incomplete preservation evidence, and missing index-direction evidence. After the issue addendum and repairs, independent re-review reported no blocking findings and confirmed each corrected assertion is non-vacuous.

## Evidence boundaries

- LOCAL: PASS — focused migration tests, full store suite, and repository gate.
- STATIC: PASS — one-file source diff, canonical-default amendment, independent re-review, rustfmt, and diff check.
- CI: NOT RUN for this branch at report time.
- LIVE: NOT RUN — no database migration outside disposable tests, backup, restore, retention, S3, OAuth, provider, deployment, or production operation occurred.

## Risk and rollback

The migration is additive and transactionally recorded once. Before release, rollback is a reverse-order revert of the one-file source commit and this one-file report commit. Once applied to a production database, rollback is forward-only through a separately reviewed migration; this package never drops the new table.
