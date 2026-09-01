# Task 11a report: backup configuration and durable YAML event validation

- **Parent:** [#85 Task 11a](https://github.com/djh00t/agent_voice/issues/85)
- **Delivery:** [PR #314](https://github.com/djh00t/agent_voice/pull/314)
- **Final follow-up:** [#336 Task 11a-R4](https://github.com/djh00t/agent_voice/issues/336)
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Verified pre-report implementation head:** `87df789eb93c79226cc3dab845903660b9e2976b`
- **Current PR/report head:** `THIS_REPORT_COMMIT` (resolve from the checked-out
  PR head after this one-file report commit)
- **PR state at authoring:** OPEN; exact rewritten report-head CI pending
- **Report head:** `THIS_REPORT_COMMIT` (resolve after this one-file commit)

This report supersedes earlier scanner-era “current” claims. Historical scanner
commits/results are historical only; the authoritative implementation is the
event reader and structural validator at the current PR head.

## Scope and serialized graph

This package covers the validated `BackupConfig` seam, safe example mapping,
pinned parser dependency, safe event-reader boundary, structural policy
validator, focused regressions, and this report. It does not implement backup
snapshots, SQLCipher, S3, retention, restore, admin UI, deployment, providers,
OAuth, or live UAT.

Serialized order: `#332 R1 (Cargo.toml) -> #333 R1b (Cargo.lock) -> #334 R2
(event reader) -> #335 R3 (event validator) -> #336 R4 (this report)`.
All are stacked on PR #314 with immediate remote-head readback; no child merge
to `origin/main` is required. R4 is the evidence gate for #85 closure. Local
green tests and PR checks are not merge, provider, deployment, or UAT proof.

## Current metadata and one-file lineage

| Stack item | Commit(s) | Owned path | Role |
| --- | --- | --- | --- |
| R1 | `0a70fca741c20d12a7c3adfeea6ce68ffceda750` | `Cargo.toml` | exact direct `unsafe-libyaml` pin |
| R1b | `f0f527cce77b677651eb59080a1ae1a3266c5c16` | `Cargo.lock` | root dependency edge |
| R2 | `f87e5994d4f17ffa838579dbf1d2d7a1454fd347`, `91dcfc48dd746e88578f734b3b50de9e567d9446`, `79c7ef4d20f423357571de01e24c475566d1c18c`, `0418d90422e63b88466b11db68f1619293a35f8c`, `87df789eb93c79226cc3dab845903660b9e2976b` | `src/config/yaml_events.rs` and four-line `src/config.rs` registration | pinned reader, RAII, exact event contracts |
| R3 | `5655d2491a7539f28070434ff1add4c4a3964583` | `src/config.rs` | structural validator and regression matrix |
| R4 | `THIS_REPORT_COMMIT` | this report only | evidence/linkage |

Each stack commit is one-file. R3 removed the old line scanner and R2's
temporary syntax-only call. Unsafe FFI exists only in `src/config/yaml_events.rs`;
`yaml_parser_delete` and `yaml_event_delete` are RAII-owned. Historical PR
heads such as `64523ac3…` and earlier are not current evidence.

## Frozen event-validator behavior

`AppConfig::load` reads the owned YAML, consumes one complete event stream,
then applies the existing semantic `BackupConfig` deserializer/validation.
The validator uses structural events and scalar style, never source lines,
indentation counts, comments, quote characters, raw slices, or
`serde_yaml::Value`.

It recognizes decoded `backup` only in the document root mapping, then direct
decoded `retention_days` and `max_age_hours` keys. Plain bytes beginning `+` or
`-`, or digit/underscore values overflowing `u64`, return fixed redacted
errors `backup.retention_days: invalid_retention` or
`backup.max_age_hours: invalid_max_age`; other type/range cases retain bounded
semantic errors. The full stream is consumed, so malformed trailing input and
duplicate documents cannot be accepted as a successful prefix.

The event path covers valid indentation, block/flow mappings, quoted keys,
comments, commas, hashes, doubled/mid-scalar apostrophes, and multiline scalar
boundaries before semantic normalization. Alias/tag events fail closed with
`config YAML event reader: unsupported_alias_or_tag`; anchors are metadata only.
Event values never enter diagnostics. Pointer/length checks and copied scalar
ownership are inside the reader boundary.

## LOCAL and TDD evidence

The original pre-implementation TDD RED run for R3 was not performed and must
not be claimed. The TDD lifecycle item remains unmet.

A tests-only **retrospective RED contract reproduction** ran against exact R2
predecessor `0418d90422e63b88466b11db68f1619293a35f8c`; it is not original TDD
evidence. The exact commands were:

```text
rtk cargo test --lib config::tests::app_config_load_rejects_event_policy_layout_matrix -- --exact --nocapture
rtk cargo test --lib config::tests::app_config_load_rejects_event_policy_comment_quote_matrix -- --exact --nocapture
rtk cargo test --lib config::tests::app_config_load_preserves_quoted_policy_lookalike_data -- --exact --nocapture
rtk cargo test --lib config::tests::app_config_load_rejects_event_alias_and_tag_bypass -- --exact --nocapture
```

These retrospective commands did not use `--locked`. Each selected one test
and reported `614 filtered out`. The layout matrix exited `101` with
`event policy fixture unexpectedly loaded: indented-root`. The comment/quote
matrix exited `101` with
`event policy fixture unexpectedly loaded: apostrophe-comment`. The alias/tag
matrix exited `101`: its assertion expected
`config YAML event reader: unsupported_alias_or_tag` but received the
historical `failed to parse YAML config`. The quoted-lookalike selector exited
`0` and passed, showing that behavior already worked at the predecessor. The
disposable worktree was removed without a commit, push, or GitHub mutation.

At pre-report implementation head `87df789…`, each exact selector below
executed one test and passed (exit `0`, `1 passed, 614 filtered out`):

```text
rtk cargo test --lib config::tests::app_config_load_rejects_event_policy_layout_matrix -- --exact --nocapture
rtk cargo test --lib config::tests::app_config_load_rejects_event_policy_comment_quote_matrix -- --exact --nocapture
rtk cargo test --lib config::tests::app_config_load_preserves_quoted_policy_lookalike_data -- --exact --nocapture
rtk cargo test --lib config::tests::app_config_load_rejects_event_alias_and_tag_bypass -- --exact --nocapture
```

`rtk cargo test --locked --lib config -- --nocapture` passed (exit `0`,
`71 passed, 544 filtered out`). These are LOCAL evidence only.

## STATIC and gate status

The owned-report `rtk git diff --check -- <report>` passed (exit `0`). The
owned `src/config.rs` formatter check is clean. `rtk cargo fmt --all -- --check`
exits `1` only for pre-existing untouched `src/pa/fakes/mail.rs`,
`src/service.rs`, and realtime files; R4 does not rewrite them. Fresh
`rtk cargo test --locked --lib config -- --nocapture` passed (exit `0`,
`71 passed, 544 filtered out`). Fresh `rtk make check` passed after the final
report reconciliation with `615` tests. Historical gates are labelled
historical and do not substantiate this head.

## CI

The historical pre-message-rewrite implementation head `0196a144…`, whose
source tree is identical to `87df789…`, reports all five checks successful:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PASS | [job 99691633421](https://github.com/djh00t/agent_voice/actions/runs/33454557970/job/99691633421) |
| Compose Config | PASS | [job 99691633568](https://github.com/djh00t/agent_voice/actions/runs/33454557970/job/99691633568) |
| Analyze (javascript-typescript) | PASS | [job 99691634023](https://github.com/djh00t/agent_voice/actions/runs/33454558043/job/99691634023) |
| Analyze (rust) | PASS | [job 99691634465](https://github.com/djh00t/agent_voice/actions/runs/33454558043/job/99691634465) |
| CodeQL aggregate | PASS | [run 99691789939](https://github.com/djh00t/agent_voice/runs/99691789939) |

CI is repository evidence only.

## Review, linkage, and lifecycle

The original report-refresh thread
[3898583253](https://github.com/djh00t/agent_voice/pull/314#discussion_r3898583253)
and source threads
[3898676483](https://github.com/djh00t/agent_voice/pull/314#discussion_r3898676483)
and
[3898676489](https://github.com/djh00t/agent_voice/pull/314#discussion_r3898676489)
were answered inline and resolved; they remain linked here as required
historical review evidence. Review of pre-rewrite report head `cfb96a4…`
opened commit-message finding
[3899731024](https://github.com/djh00t/agent_voice/pull/314#discussion_r3899731024)
and implementation-head-label finding
[3899731030](https://github.com/djh00t/agent_voice/pull/314#discussion_r3899731030).
The final three commits now use the required `Why`, `What`, and structured
`Refs` sections, and this report labels `87df789…` as the verified pre-report
implementation head. These two findings remain unresolved at authoring until
the rewritten head is pushed, answered inline, and read back.
PR #314 must retain `Closes #332`, `Closes #333`, `Closes #334`, `Closes #335`,
`Closes #336`, and `Closes #85`, plus `Refs #314`. #85 and PR #314 remain open
until human review/merge authority is satisfied. Rollback is reverting this
one report-file commit after confirming its remote predecessor.

## LIVE: NOT RUN

S3 credentials/uploads/listings, retention deletion, freshness/alerts, restore,
OAuth consent/refresh, Graph, Gmail, Google Calendar, SIP, OpenAI, deployment,
Kubernetes, browser/admin access, authenticated UAT, live calls, email delivery,
and production filesystem/configuration are all **NOT RUN** and **NOT CLAIMED**.
No local test or green CI result proves any live behavior.

## Acceptance mapping

| Requirement | Status |
| --- | --- |
| Current event-validator head and R1-R4 ancestry | PASS: verified metadata/lineage above |
| Structural layout/comment/quote/alias coverage | PASS LOCAL/CI: four exact selectors, one test each |
| Honest original RED evidence | UNMET: not performed; disclosed without substitution |
| Retrospective predecessor contract reproduction | PASS: three required failures and one preserved behavior reproduced |
| RAII boundary and fixed redacted errors | PASS LOCAL/STATIC: source and focused tests |
| Locked full test, formatter, diff, make-check evidence | PARTIAL: focused locked tests pass; unrelated fmt drift; full completion not claimed |
| Review/linkage authority | PASS: zero source threads; one report thread linked; no state mutation |
| Provider/deployment/live behavior | NOT RUN |

**Package status:** the report is ready for final verification and delivery.
The original TDD lifecycle gap remains explicit and cannot be repaired
retroactively. The report thread remains unresolved until this one-file report
commit is pushed, answered inline, and independently reviewed.
