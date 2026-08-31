# Task 11a report: backup configuration and contract

- **Issue:** [#85](https://github.com/djh00t/agent_voice/issues/85)
- **Feature:** [#62](https://github.com/djh00t/agent_voice/issues/62)
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Base SHA:** `a20a28be3be37c84cbe5046415497b7053dd8906` (`origin/main` after
  `rtk git fetch origin main`)
- **Implementation head SHA:** `5503c19c3769adfdabe670846d8b178891bd59c3`
  (latest source/config implementation commit before this report-only change)
- **PR:** [#314](https://github.com/djh00t/agent_voice/pull/314)
- **Branch:** `codex/agent-voice-issue-85`
- **Evidence worktree:** `/private/tmp/agent-voice-issue-85-e` (removed after
  delivery)
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

### Focused GREEN evidence

At implementation head `8a4173bfc360c9cb8fd9a0a3cda81c9977743697`, each exact
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

At implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`, the
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

The repository gate passed after installing the existing website lockfile
dependencies in the disposable worktree:

```text
Command: rtk npm ci (from website/)
Exit: 0
Result: 1,276 packages installed; npm reported existing audit/deprecation
        notices and no manifest or lockfile changed.

Command: rtk make check
Exit: 0
Result: cargo test 576 passed, 0 failed; integration suites passed with
        6, 18, 233, 41, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
```

At implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`, the
repository gate was repeated after the broadened regression:

```text
Command: rtk make check
Exit: 0
Result: cargo test 577 passed, 0 failed; integration suites passed with
        6, 18, 233, 42, 3, 3, 3, 3, and 19 tests; doc-tests 3 passed;
        cargo clippy, cargo doc, and Docusaurus build completed successfully.
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

The implementation range through the current source head was inspected with:

```text
Command: rtk git diff --name-status origin/main...5503c19c3769adfdabe670846d8b178891bd59c3
Exit: 0
Result: exactly the three owned paths src/config.rs, config/agent_voice.example.yaml,
        and .superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md.
```

The nine delivery commits before this report are each one-file commits;
`rtk git log --stat origin/main..5503c19c3769adfdabe670846d8b178891bd59c3`
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

The repository-wide formatter remains a pre-existing, out-of-scope issue:

```text
Command: rtk cargo fmt --all -- --check
Exit: 1
Result: drift only in untouched src/pa/fakes/mail.rs, src/service.rs, and
        tests/../src/realtime/server_audio_events.rs; owned src/config.rs
        passes the scoped formatter check above.
```

## CI

At implementation head `5503c19c3769adfdabe670846d8b178891bd59c3`, PR #314
reported all five checks green:

| Check | Result | Evidence |
| --- | --- | --- |
| Quality Gates | PASS | [CI job 99559816552](https://github.com/djh00t/agent_voice/actions/runs/33413883982/job/99559816552) |
| Compose Config | PASS | [CI job 99559816242](https://github.com/djh00t/agent_voice/actions/runs/33413883982/job/99559816242) |
| Analyze (javascript-typescript) | PASS | [CodeQL job 99559816370](https://github.com/djh00t/agent_voice/actions/runs/33413884007/job/99559816370) |
| Analyze (rust) | PASS | [CodeQL job 99559816427](https://github.com/djh00t/agent_voice/actions/runs/33413884007/job/99559816427) |
| CodeQL aggregate | PASS | [aggregate run 99560043383](https://github.com/djh00t/agent_voice/runs/99560043383) |

This report-only commit will create a new PR workflow run; no status for that
new report head is claimed until GitHub reports it. CI is repository evidence
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
four review threads were resolved at this capture.

## Acceptance mapping

| Contract | Evidence | Status |
| --- | --- | --- |
| `BackupConfig` exposes the frozen public fields and safe defaults | `backup_config_defaults_disabled_and_safe`; source review | PASS (LOCAL/STATIC) |
| Exactly eight overrides win over YAML and reject blank/malformed values atomically | `backup_config_env_overrides_are_strict_and_normalized`; enabled negative selector | PASS (LOCAL) |
| Stable error classes never echo raw values or secret-shaped fields | secret-field selectors, including common credential names; redaction assertions; source review | PASS (LOCAL/STATIC) |
| Bucket, region, prefix, endpoint, policy, and scratch path fail closed | destination and required-negative selectors | PASS (LOCAL) |
| Empty endpoint userinfo is rejected and non-default production ports remain disallowed | empty-userinfo selector; endpoint source review; frozen addendum | PASS (LOCAL/STATIC) |
| Explicit test-only loopback HTTP is isolated from production validation | `backup_config_snapshot_and_runtime_handoffs` | PASS (LOCAL) |
| Example mapping remains disabled-safe and exact | snapshot/runtime handoff selector | PASS (LOCAL) |

**Package status:** implementation and evidence are ready for review; the live
issue label remains `status:in-progress` at this capture. CI at implementation
head `5503c19c3769adfdabe670846d8b178891bd59c3` is green; this report-only
commit's new CI run is pending until GitHub reports it. Live, deployment,
merge, and approval evidence remain separate gates.
