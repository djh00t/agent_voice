# Task 11a report: backup configuration and contract

- **Issue:** [#85](https://github.com/djh00t/agent_voice/issues/85)
- **Feature:** [#62](https://github.com/djh00t/agent_voice/issues/62)
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Base SHA:** `a20a28be3be37c84cbe5046415497b7053dd8906` (`origin/main` after
  `rtk git fetch origin main`)
- **Implementation head SHA:** `10e1454e9ce8e143a29f1039ff3e0e13920f23b0`
  (latest source/config implementation commit before this report-only change)
- **Pre-report head SHA:** `3e7fff1bfc305c221665e5862adbf9fc366d1142`
  (previous source/report tip before this report-only refresh)
- **Report head SHA:** `THIS_REPORT_COMMIT` (self-reference; resolve with
  `rtk git rev-parse HEAD` after checkout)
- **Report lineage:** `7ca2da030b775502bb3b60c53726cf919869d80e`,
  `3e7fff1bfc305c221665e5862adbf9fc366d1142` (prior reports), and
  `THIS_REPORT_COMMIT` (self-reference; resolve with `rtk git rev-parse HEAD`)
- **PR:** [#314](https://github.com/djh00t/agent_voice/pull/314)
- **Branch:** `codex/agent-voice-issue-85`
- **Evidence worktree:** `/private/tmp/agent-voice-pr314-report-refresh` (removed
  after delivery)
- **Prerequisite:** #218 is closed. Its `AgentApiConfig.oauth` field and
  post-environment OAuth normalization handoff were re-read from `origin/main`
  at the base SHA before implementation.

## Scope and ownership

This package owns only the backup configuration seam and its evidence:

- `src/config.rs`: public `BackupConfig`, safe defaults, strict `BACKUP_*`
  environment overrides, cloned final validation, endpoint allowlisting,
  stable error classes, and focused `config::tests` selectors.
- `config/agent_voice.example.yaml`: the backup mapping with disabled-safe
  defaults.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md`: this report.

No snapshot production, SQLCipher envelope, S3 transport, retention execution,
durable attempt history, health/alerts, restore, CLI, browser/admin surface,
deployment, provider, or live-credential behavior was added.

## Frozen configuration contract

`AppConfig` owns a serde-defaulted `BackupConfig` with public fields
`enabled`, `bucket`, `prefix`, `region`, `endpoint`, `retention_days`,
`temp_dir`, and `max_age_hours`. The disabled-safe defaults are:

| Field | Default |
| --- | --- |
| `enabled` | `false` |
| `bucket` | empty |
| `prefix` | `backups` |
| `region` | empty |
| `endpoint` | `None` |
| `retention_days` | `30` |
| `temp_dir` | `./backup-tmp` |
| `max_age_hours` | `24` |

Only the eight documented `BACKUP_*` environment keys are applied after YAML;
outer whitespace and one matching quote pair are normalized, and present blank
or malformed values fail atomically. Validation errors use only the frozen
field/code classes `missing_required`, `invalid_bucket`, `invalid_region`,
`invalid_prefix`, `invalid_endpoint`, `invalid_retention`, `invalid_max_age`,
`invalid_temp_dir`, and `secret_field_rejected`. Unknown secret-shaped YAML
fields—including `master_key`, `raw_secret`, `raw_key`, `secret`, `secret_key`,
`secret_access_key`, `access_token`, and `credentials` plus separator-aware
compound variants—return `backup: secret_field_rejected` without echoing a key
value. Ordinary unknown keys retain serde's generic unknown-field error.
YAML policy values also use bounded intermediates so wrong-type and
out-of-range `retention_days` and `max_age_hours` inputs map to their frozen
field/code errors without echoing raw values. The direct `BackupConfig` YAML
deserializer receives semantic values: a lexical `+30` is normalized by
`serde_yaml` to the same numeric value as `30`, so direct deserialization cannot
enforce a lexical-sign rule. The production `AppConfig::load` file path adds a
raw-lexeme guard before YAML deserialization for signed and oversized decimal
policy literals. That guard intentionally covers standard block mappings and
inline simple-flow mappings such as `backup: { retention_days: +30 }`; it is
not a general YAML parser and does not claim coverage for complex flow,
multiline, alias, tag, or equivalent non-file deserialization paths.

Configured production endpoints are HTTPS origins with DNS hosts, no userinfo,
query, fragment, or non-default port. HTTP loopback is accepted only through
the explicit test-only validator. `BackupConfig` contains no key material,
token, credential, or secret value, and its `Debug` output uses fixed presence
markers for sensitive-shaped values.

## LOCAL

### TDD RED evidence

The required missing-contract selector ran before the initial implementation:

```text
Command: rtk cargo test --lib config::tests::backup_config_contract -- --exact --nocapture
Exit: 101
Result: 11 compile errors (0 warnings), including missing BackupConfig and
        AppConfig.backup; this was a true missing-contract failure.
```

The prior remediation tests also demonstrated their original defects before
their source changes:

```text
Command: rtk cargo test --lib config::tests::backup_config_enabled_override_rejects_blank_and_malformed -- --exact --nocapture
Exit: 101
Result: blank/malformed BACKUP_ENABLED returned invalid_enabled instead of the
        frozen backup.enabled: missing_required class.

Command: rtk cargo test --lib config::tests::backup_config_rejects_secret_shaped_unknown_yaml_fields -- --exact --nocapture
Exit: 101
Result: master_key produced serde's generic unknown-field error instead of
        backup: secret_field_rejected.

Command: rtk cargo test --lib config::tests::backup_config_rejects_empty_endpoint_userinfo -- --exact --nocapture
Exit: 101
Result: https://@s3.example.test/ was accepted and unwrap_err() received Ok(()).

Command: rtk cargo test --lib config::tests::backup_config_rejects_required_negative_fixtures -- --exact --nocapture
Exit: 0
Result: NUL prefix, endpoint query, max-age zero, and max-age overflow were
        already rejected; this test supplied regression coverage without a
        behavior change for those cases.
```

### Remediation D RED evidence

The common-secret regression was authored for the change delivered at
implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`; it did not
exist at the earlier implementation head `8a4173bfc360c9cb8fd9a0a3cda81c9977743697`.
The RED run occurred on its immediate predecessor
`821e9f7900b0d44fb6be0e6c6e27800a08cda9c5`, before the source and test change
was committed:

```text
Command: rtk cargo test --lib config::tests::backup_config_rejects_common_secret_shaped_unknown_yaml_fields -- --exact --nocapture
Exit: 101
Result: 0 passed, 1 failed, and 576 filtered out because the four common
        credential names fell through to serde's generic unknown-field error.
```

### Remediation E RED evidence

The YAML policy-error regression was authored for the change delivered at
implementation head `d6e03cb8dc476f499e5cdb6d3aea5525be03c785`; it did not
exist at the preceding report head `ea4f0916ba9386ee34c25bf95c4baad6b1be8128`.
The RED run occurred on that predecessor before the source and test change was
committed:

```text
Command: rtk cargo test --lib config::tests::backup_config_yaml_policy_errors_are_frozen_and_redacted -- --exact --nocapture
Exit: 101
Result: 0 passed, 1 failed, and 577 filtered out because retention_days: 0
        deserialized successfully instead of returning the frozen error class.
```

### Remediation F lexical-load evidence

The production file-load regression was added in
`c8ddc77f45931a02ad7b7f82ccb02488858034c5`. It exercises signed and oversized
retention and freshness literals through `AppConfig::load`, where the raw file
contents are available before `serde_yaml` turns them into semantic values.
The guard is deliberately bounded to standard block `backup:` mappings and
inline simple-flow mappings; it is not a general YAML lexer/parser. Direct
`serde_yaml::from_str::<BackupConfig>` remains unable to distinguish `+30` from
`30` after semantic parsing, so the direct plus-sign case is documented as a
limitation rather than claimed as covered by this production-load guard.

### Follow-up G single-quoted-key evidence

The follow-up implementation at `726392c40ed4b22cfcecf8d91871337c5b893f94`
extends the same bounded guard to single-quoted policy keys and whitespace
around quoted keys and colons. Both standard-block and inline simple-flow
`AppConfig::load` regressions cover signed and oversized values without raw
value echo.

### Follow-up H trailing-comment flow evidence

The historical follow-up implementation at `1491c1316f2195323af3ffc5c9455894fe654662`
strips a trailing YAML comment before extracting the outer simple-flow mapping.
This closes the remaining production-load gap for
`backup: { retention_days: +30 } # policy` without broadening the bounded
lexical scanner. The regression asserts the frozen redacted
`backup.retention_days: invalid_retention` error and does not echo `+30`.

### Follow-up I quoted-hash flow evidence

The current implementation at `10e1454e9ce8e143a29f1039ff3e0e13920f23b0`
makes the bounded comment scan quote-aware. A `#` inside a single- or
double-quoted scalar is retained as YAML data, while an unquoted `#` still
starts the trailing comment. The regression places `temp_dir: "backup#tmp"`
before `retention_days: +30` and proves the signed policy literal is rejected
with `backup.retention_days: invalid_retention` without echoing either value.

### Focused GREEN evidence

At historical implementation head `8a4173bfc360c9cb8fd9a0a3cda81c9977743697`, each exact
selector below exited `0`, executed exactly one listed test, and reported
`1 passed, 575 filtered out`:

The common-secret regression did not yet exist at this historical head and is
therefore not included in this selector list or its 576-test module count.

```text
rtk cargo test --lib config::tests::backup_config_contract -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_defaults_disabled_and_safe -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_env_overrides_are_strict_and_normalized -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_rejects_destination_escape_and_secrets -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_snapshot_and_runtime_handoffs -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_enabled_override_rejects_blank_and_malformed -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_rejects_secret_shaped_unknown_yaml_fields -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_rejects_empty_endpoint_userinfo -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_rejects_required_negative_fixtures -- --exact --nocapture
```

The complete config module also passed:

```text
Command: rtk cargo test --lib config -- --nocapture
Exit: 0
Result: 44 passed, 532 filtered out (576 config tests available).
```

At historical implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`, the
broadened secret-field regression was present and passed, alongside the
complete config module:

```text
Command: rtk cargo test --lib config::tests::backup_config_rejects_common_secret_shaped_unknown_yaml_fields -- --exact --nocapture
Exit: 0
Result: 1 passed, 576 filtered out; all four requested credential names return
        backup: secret_field_rejected without the sentinel, while object_key
        and monkey retain generic unknown-field errors.

Command: rtk cargo test --lib config -- --nocapture
Exit: 0
Result: 45 passed, 532 filtered out (577 tests available).
```

At historical implementation head `d6e03cb8dc476f499e5cdb6d3aea5525be03c785`, the YAML
policy-error regression was present and passed, alongside the complete config
module:

```text
Command: rtk cargo test --lib config::tests::backup_config_yaml_policy_errors_are_frozen_and_redacted -- --exact --nocapture
Exit: 0
Result: 1 passed, 577 filtered out; wrong-type, zero, and overflow policy
        values return the exact frozen errors without echoing raw values.

Command: rtk cargo test --lib config -- --nocapture
Exit: 0
Result: 46 passed, 532 filtered out (578 tests available).
```

The repository gate passed after installing the existing website lockfile
dependencies in the disposable worktree:

```text
Command: rtk run 'cd website && rtk npm ci'
Exit: 0
Result: 1,276 packages installed; npm reported existing audit/deprecation
        notices and no manifest or lockfile changed.

Command: rtk make check
Exit: 0
Result: cargo test 576 passed, 0 failed; integration suites passed with
        6, 18, 233, 41, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At historical implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`, the
repository gate was repeated after the broadened regression:

```text
Command: rtk make check
Exit: 0
Result: cargo test 577 passed, 0 failed; integration suites passed with
        6, 18, 233, 42, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At historical implementation head `d6e03cb8dc476f499e5cdb6d3aea5525be03c785`, the
repository gate was repeated after the policy-error fix:

```text
Command: rtk make check
Exit: 0
Result: cargo test 578 passed, 0 failed; integration suites passed with
        6, 18, 233, 43, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At historical implementation head `c8ddc77f45931a02ad7b7f82ccb02488858034c5`, the
production file-load and existing scalar-redaction regressions passed:

```text
Command: rtk cargo test --lib config::tests::app_config_load_rejects_signed_and_oversized_backup_policy_literals -- --exact --nocapture
Exit: 0
Result: 1 passed, 580 filtered out; signed and oversized policy literals were
        rejected through AppConfig::load without echoing the raw literal.

Command: rtk cargo test --lib config::tests::backup_config_yaml_enabled_type_errors_are_frozen_and_redacted -- --exact --nocapture
Exit: 0
Result: 1 passed, 580 filtered out; malformed/non-boolean enabled values map to
        backup.enabled: missing_required without the sentinel.

Command: rtk cargo test --lib config::tests::backup_config_yaml_scalar_type_errors_are_frozen_and_redacted -- --exact --nocapture
Exit: 0
Result: 1 passed, 580 filtered out; analogous scalar conversion failures map to
        frozen field/code errors without the sentinel.

Command: rtk cargo test --lib config -- --nocapture
Exit: 0
Result: 49 passed, 532 filtered out (581 config-module tests available).

Command: rtk run 'cd website && rtk npm ci'
Exit: 0
Result: 1,276 packages installed; npm reported existing audit/deprecation
        notices and no manifest or lockfile changed.

Command: rtk make check
Exit: 0
Result: cargo test 581 passed, 0 failed; integration suites passed with
        6, 18, 233, 46, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At historical implementation head `726392c40ed4b22cfcecf8d91871337c5b893f94`, the
single-quoted-key regressions and full gate passed:

```text
Command: rtk cargo test --lib config::tests::app_config_load_rejects_single_quoted_block_policy_literals -- --exact --nocapture
Exit: 0
Result: 1 passed, 582 filtered out; standard-block signed and oversized
        single-quoted keys were rejected without raw value echo.

Command: rtk cargo test --lib config::tests::app_config_load_rejects_single_quoted_flow_policy_literals -- --exact --nocapture
Exit: 0
Result: 1 passed, 582 filtered out; simple-flow signed and oversized
        single-quoted keys were rejected without raw value echo.

Command: rtk cargo test --lib config -- --nocapture
Exit: 0
Result: 51 passed, 532 filtered out (583 config-module tests available).

Command: rtk run 'cd website && rtk npm ci'
Exit: 0
Result: 1,276 packages installed; npm reported existing audit/deprecation
        notices and no manifest or lockfile changed.

Command: rtk make check
Exit: 0
Result: cargo test 583 passed, 0 failed; integration suites passed with
        6, 18, 233, 48, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At historical implementation head `1491c1316f2195323af3ffc5c9455894fe654662`, the
trailing-comment flow regression and full local gate passed:

```text
Command: rtk run 'cargo test --lib config::tests::app_config_load_rejects_flow_policy_literal_followed_by_comment -- --exact --nocapture'
Exit: 0
Result: 1 passed, 583 filtered out; the signed flow-mapping literal with a
       trailing YAML comment returns the frozen error without echoing `+30`.

Command: rtk run 'cargo test --lib config::tests -- --nocapture'
Exit: 0
Result: 46 passed, 0 failed; the config and adjacent admin-configuration
       tests passed.

Command: rtk make check
Exit: 0
Result: cargo test 584 passed, 0 failed; integration suites passed with
        6, 18, 233, 48, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At current implementation head `10e1454e9ce8e143a29f1039ff3e0e13920f23b0`,
the quoted-hash ordering regression and the final local gate passed:

```text
Command: rtk cargo test --lib config::tests::app_config_load_rejects_signed_policy_after_quoted_hash -- --exact --nocapture
Exit: 0
Result: 1 passed; a quoted hash remains scalar data and the later +30 policy
        literal returns the frozen redacted retention error.

Command: rtk cargo test --lib config -- --nocapture
Exit: 0
Result: 53 passed, 0 failed.

Command: rtk make check
Exit: 0
Result: cargo test 585 passed, 0 failed; integration suites, doc-tests,
        cargo clippy, cargo doc, and Docusaurus completed successfully.
```

Configuration normalization is clone-then-assign. Failed parsing or validation
therefore publishes no partial config and performs no filesystem, clock,
socket, network, provider, database, or token action. Re-loading identical
YAML/environment maps produces equal typed values.

## STATIC

The owned source formatter and whitespace checks passed:

```text
Command: rtk rustfmt --edition 2024 --check src/config.rs
Exit: 0

Command: rtk git diff --check
Exit: 0
```

The implementation range through source head
`10e1454e9ce8e143a29f1039ff3e0e13920f23b0` was inspected with:

```text
Command: rtk git diff --name-status origin/main...10e1454e9ce8e143a29f1039ff3e0e13920f23b0
Exit: 0
Result: exactly the three owned paths src/config.rs, config/agent_voice.example.yaml,
        and .superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md.
```

The twenty-two delivery commits before this report are each one-file commits;
`rtk git log --stat origin/main..3e7fff1bfc305c221665e5862adbf9fc366d1142`
returned the following path boundaries:

| Commit | Path | Change |
| --- | --- | --- |
| `29afdf801b218c3f22649f7472a98437ae5bdd9e` | `src/config.rs` | Add validated backup settings. |
| `e9af9c1a2ebd38057e4cff666386c05c47e78979` | `config/agent_voice.example.yaml` | Document backup defaults. |
| `4d78c4021931a2fa9973f975ed53e3f44c063df5` | `src/config.rs` | Satisfy backup test lint. |
| `7fa5c3c4950998977f4ebf36d39040ce8e8cc23f` | `src/config.rs` | Name backup policy errors precisely. |
| `d746116312233b3ede61b9f0aff9c9b753c3b11d` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Record initial package evidence. |
| `2adfb72bc65f3adcc5986897c1afaa4499776cec` | `src/config.rs` | Stabilize frozen rejection errors. |
| `8a4173bfc360c9cb8fd9a0a3cda81c9977743697` | `src/config.rs` | Reject empty endpoint userinfo. |
| `821e9f7900b0d44fb6be0e6c6e27800a08cda9c5` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Refresh full-SHA, LOCAL/STATIC/CI/LIVE evidence. |
| `5503c19c3769adfdabe670846d8b178891bd59c3` | `src/config.rs` | Classify common secret-shaped keys and add focused regressions. |
| `b9cb959660312b9ea1fdec16726312c8948a507f` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Record current implementation, regression, review, and CI evidence. |
| `ea4f0916ba9386ee34c25bf95c4baad6b1be8128` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Clarify historical provenance for the broadened secret-key regression. |
| `d6e03cb8dc476f499e5cdb6d3aea5525be03c785` | `src/config.rs` | Freeze YAML policy errors and add wrong-type/overflow regressions. |
| `e3fda1fe1810c2a9bef23f5515a8bfd67193ab52` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Record YAML policy-error repair evidence and provenance. |
| `8f1ce09125f412623307dd8420bfb144f5b87e2e` | `src/config.rs` | Map malformed YAML scalar fields to frozen redacted errors and add adversarial regressions. |
| `c8ddc77f45931a02ad7b7f82ccb02488858034c5` | `src/config.rs` | Reject signed and oversized policy literals on the production file-load path. |
| `fbc245619cf998dd8d1ea5fa3ce0a3bfd2014b72` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Finalize backup evidence before the quoted-key repair. |
| `726392c40ed4b22cfcecf8d91871337c5b893f94` | `src/config.rs` | Match single-quoted policy keys and add block/flow regressions. |
| `cd7d4512999e53de41a15ede55d5bb5df43fcc69` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Record quoted policy-key repair evidence and review state. |
| `1491c1316f2195323af3ffc5c9455894fe654662` | `src/config.rs` | Scan flow policy mappings before trailing comments and add the regression. |
| `7ca2da030b775502bb3b60c53726cf919869d80e` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Record trailing-comment repair evidence. |
| `10e1454e9ce8e143a29f1039ff3e0e13920f23b0` | `src/config.rs` | Make flow-comment detection quote-aware and add the quoted-hash regression. |
| `3e7fff1bfc305c221665e5862adbf9fc366d1142` | `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md` | Label historical evidence heads before the final report refresh. |

The repository-wide formatter remains a pre-existing, out-of-scope issue:

```text
Command: rtk cargo fmt --all -- --check
Exit: 1
Result: drift only in untouched src/pa/fakes/mail.rs, src/service.rs, and
        tests/../src/realtime/server_audio_events.rs; owned src/config.rs
        passes the scoped formatter check above.
```

## CI

The prior implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`
reported all five checks green:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PASS | [CI job 99559816552](https://github.com/djh00t/agent_voice/actions/runs/33413883982/job/99559816552) |
| Compose Config | PASS | [CI job 99559816242](https://github.com/djh00t/agent_voice/actions/runs/33413883982/job/99559816242) |
| Analyze (javascript-typescript) | PASS | [CodeQL job 99559816370](https://github.com/djh00t/agent_voice/actions/runs/33413884007/job/99559816370) |
| Analyze (rust) | PASS | [CodeQL job 99559816427](https://github.com/djh00t/agent_voice/actions/runs/33413884007/job/99559816427) |
| CodeQL aggregate | PASS | [aggregate run 99560043383](https://github.com/djh00t/agent_voice/runs/99560043383) |

At historical implementation-head snapshot
`d6e03cb8dc476f499e5cdb6d3aea5525be03c785`, the fresh CI run is partially
complete:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PENDING | [CI job 99574695696](https://github.com/djh00t/agent_voice/actions/runs/33418456370/job/99574695696) |
| Compose Config | PASS | [CI job 99574695445](https://github.com/djh00t/agent_voice/actions/runs/33418456370/job/99574695445) |
| Analyze (javascript-typescript) | PASS | [CodeQL job 99574696394](https://github.com/djh00t/agent_voice/actions/runs/33418456384/job/99574696394) |
| Analyze (rust) | PENDING | [CodeQL job 99574696812](https://github.com/djh00t/agent_voice/actions/runs/33418456384/job/99574696812) |
| CodeQL aggregate | NEUTRAL / SKIPPING | [aggregate run 99574940345](https://github.com/djh00t/agent_voice/runs/99574940345) |

At historical implementation-head snapshot
`c8ddc77f45931a02ad7b7f82ccb02488858034c5`, all five checks are green:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PASS | [CI job 99591892288](https://github.com/djh00t/agent_voice/actions/runs/33423660836/job/99591892288) |
| Compose Config | PASS | [CI job 99591892137](https://github.com/djh00t/agent_voice/actions/runs/33423660836/job/99591892137) |
| Analyze (javascript-typescript) | PASS | [CodeQL job 99591894810](https://github.com/djh00t/agent_voice/actions/runs/33423660737/job/99591894810) |
| Analyze (rust) | PASS | [CodeQL job 99591895091](https://github.com/djh00t/agent_voice/actions/runs/33423660737/job/99591895091) |
| CodeQL aggregate | PASS | [aggregate run 99592192231](https://github.com/djh00t/agent_voice/runs/99592192231) |

At implementation head `726392c40ed4b22cfcecf8d91871337c5b893f94`, the fresh
PR run was in progress at report capture:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | IN_PROGRESS | [CI job 99601432890](https://github.com/djh00t/agent_voice/actions/runs/33426556080/job/99601432890) |
| Compose Config | PASS | [CI job 99601432675](https://github.com/djh00t/agent_voice/actions/runs/33426556080/job/99601432675) |
| Analyze (javascript-typescript) | IN_PROGRESS | [CodeQL job 99601432730](https://github.com/djh00t/agent_voice/actions/runs/33426556101/job/99601432730) |
| Analyze (rust) | IN_PROGRESS | [CodeQL job 99601432840](https://github.com/djh00t/agent_voice/actions/runs/33426556101/job/99601432840) |
| CodeQL aggregate | NOT REPORTED | not yet emitted while analysis jobs run |

At implementation head `1491c1316f2195323af3ffc5c9455894fe654662`, the fresh
PR run `33429310808` completed successfully. Its repository CI jobs were green;
the paired CodeQL run had JavaScript analysis green and Rust analysis pending at
report capture:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PASS | [CI job 99610499540](https://github.com/djh00t/agent_voice/actions/runs/33429310808/job/99610499540) |
| Compose Config | PASS | [CI job 99610499743](https://github.com/djh00t/agent_voice/actions/runs/33429310808/job/99610499743) |
| Analyze (javascript-typescript) | PASS | [CodeQL job 99610499550](https://github.com/djh00t/agent_voice/actions/runs/33429310805/job/99610499550) |
| Analyze (rust) | PENDING | [CodeQL job 99610500087](https://github.com/djh00t/agent_voice/actions/runs/33429310805/job/99610500087) |
| CodeQL aggregate | SKIPPING | [aggregate run 99610724526](https://github.com/djh00t/agent_voice/runs/99610724526) |

At current source/report head `3e7fff1bfc305c221665e5862adbf9fc366d1142`,
CI run `33432612484` passed after the failed SQLCipher initialization job was
rerun unchanged. The original failure affected unrelated admin-config store
tests; the rerun completed the same repository gate successfully.

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PASS | [rerun job 99622063367](https://github.com/djh00t/agent_voice/actions/runs/33432612484/job/99622063367) |
| Compose Config | PASS | [rerun job 99622065343](https://github.com/djh00t/agent_voice/actions/runs/33432612484/job/99622065343) |
| Analyze (javascript-typescript) | PASS | [CodeQL job 99621347479](https://github.com/djh00t/agent_voice/actions/runs/33432612521/job/99621347479) |
| Analyze (rust) | PASS | [CodeQL job 99621347698](https://github.com/djh00t/agent_voice/actions/runs/33432612521/job/99621347698) |
| CodeQL aggregate | PASS | [aggregate run 99621579644](https://github.com/djh00t/agent_voice/runs/99621579644) |

The final report-only commit uses `THIS_REPORT_COMMIT` for its unavoidable
self-reference and will trigger a fresh workflow. CI is repository evidence
only and does not substitute for live-provider or deployment evidence.

## LIVE

The following are explicitly **NOT RUN** and **NOT CLAIMED** by this package:

- S3-compatible provider credentials, network calls, uploads, listings, or
  retention deletion.
- SQLCipher snapshot production, envelope verification, restore, or target
  installation.
- Durable backup-attempt history, freshness evaluation, health metrics, alert
  routing, or scheduled daily execution.
- OAuth, SIP, OpenAI, Gmail, Outlook, Microsoft Graph, deployment, Kubernetes,
  production filesystem, or production configuration behavior.
- Browser/admin surface, authenticated UAT, live calls, email delivery, or
  provider response handling.

No local test, rendered configuration, health response, or green CI check is
evidence for any of those live claims.

## Review, lifecycle, and rollback

Issue #85 was moved from `status:blocked` to `status:in-progress` after #218
was confirmed closed; the pickup comment records the prerequisite/base/SHA and
true RED evidence. The report-only delivery remains one file and one logical
change with `Refs: issue: #85`.

The delivering PR contains exactly `Closes #85` plus `Refs #62`, `Refs #218`,
`Refs #107`, `Refs #109`, `Refs #110`, `Refs #111`, `Refs #112`, `Refs #113`,
and `Refs #120`; it does not close the feature tracker, prerequisite, or any
downstream handoff. Rollback is a code revert of the owned source/example/report
commits; no remote object, database, token, or restore target is touched.

The initial automated review also contained three stale threads. The current
commit history proves the atomicity and multiline-template findings false, and
the authoritative endpoint contract intentionally rejects production
non-default ports. Those threads are reconciled inline with exact evidence and
resolved separately from this report commit. The common secret-field finding
(`3896205037`) was answered by `3896329278` against implementation head
`5503c19c3769adfdabe670846d8b178891bd59c3` and its thread was resolved; all
four review threads were resolved at that capture. The YAML policy-error
finding (`3896592605`) was answered by `3896701634` against repair head
`d6e03cb8dc476f499e5cdb6d3aea5525be03c785` and its thread was resolved; all
five review threads were resolved at that capture. The malformed-enabled
finding (`3896778384`) was answered by `3896892063` against repair head
`8f1ce09125f412623307dd8420bfb144f5b87e2e` and its thread was resolved; all
six review threads were resolved at the c8 implementation-head capture.
The single-quoted-key finding (`3897240948`) was answered by `3897325896`
against `726392c40ed4b22cfcecf8d91871337c5b893f94` and its thread was resolved;
all seven review threads were resolved at this capture.
The trailing-comment flow finding (`3897415385`) was answered by `3897560105`
against `1491c1316f2195323af3ffc5c9455894fe654662` and its thread was resolved;
all eight review threads were resolved at this capture.
The quoted-hash flow finding (`3897715118`) and stale historical-head-label
finding (`3897715125`) were answered against `10e1454e9ce8e143a29f1039ff3e0e13920f23b0`
and `3e7fff1bfc305c221665e5862adbf9fc366d1142`; both threads were resolved. All
ten review threads were resolved at the final evidence capture.

## Acceptance mapping

| Contract | Evidence | Status |
| --- | --- | --- |
| `BackupConfig` exposes the frozen public fields and safe defaults | `backup_config_defaults_disabled_and_safe`; source review | PASS (LOCAL/STATIC) |
| Exactly eight overrides win over YAML and reject blank/malformed values atomically | `backup_config_env_overrides_are_strict_and_normalized`; enabled negative selector | PASS (LOCAL) |
| Stable error classes never echo raw values or secret-shaped fields | secret-field selectors, YAML enabled/scalar selectors, redaction assertions, and source review | PASS (LOCAL/STATIC) |
| Bucket, region, prefix, endpoint, policy, and scratch path fail closed | destination, required-negative, and YAML policy-error selectors | PASS (LOCAL) |
| YAML wrong-type and out-of-range policy values map to frozen redacted errors | `backup_config_yaml_policy_errors_are_frozen_and_redacted` | PASS (LOCAL/STATIC) |
| Production file loads reject signed and oversized policy literals before semantic YAML parsing | `app_config_load_rejects_signed_and_oversized_backup_policy_literals`, single-quoted block/flow selectors, trailing-comment and quoted-hash flow selectors, and raw-lexeme guard source review | PASS (LOCAL/STATIC; standard block/simple-flow scope) |
| Direct `BackupConfig` parsing rejects lexical plus signs | `serde_yaml` semantic-number boundary | LIMITATION (documented; not claimed) |
| Empty endpoint userinfo is rejected and non-default production ports remain disallowed | empty-userinfo selector; endpoint source review; frozen addendum | PASS (LOCAL/STATIC) |
| Explicit test-only loopback HTTP is isolated from production validation | `backup_config_snapshot_and_runtime_handoffs` | PASS (LOCAL) |
| Example mapping remains disabled-safe and exact | snapshot/runtime handoff selector | PASS (LOCAL) |

**Package status:** implementation and LOCAL/STATIC evidence are ready for
review; issue #85 is labelled `status:review` at final capture. The exact
pre-report head `3e7fff1bfc305c221665e5862adbf9fc366d1142` has all five checks green after
the unchanged failed-job rerun. The final report-only commit will trigger a new
workflow; its own head status is not claimed until GitHub reports it. Live,
deployment, merge, and approval evidence remain separate gates.
