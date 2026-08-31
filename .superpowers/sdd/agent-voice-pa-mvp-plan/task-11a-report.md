# Task 11a report: backup configuration and contract

- **Issue:** [#85](https://github.com/djh00t/agent_voice/issues/85)
- **Feature:** [#62](https://github.com/djh00t/agent_voice/issues/62)
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-issue-85`
- **Branch:** `codex/agent-voice-issue-85`
- **Base:** `a20a28b` (`origin/main` after `rtk git fetch origin main`)
- **Prerequisite:** #218 is closed; its `AgentApiConfig.oauth` field and
  post-environment OAuth normalization handoff were re-read from `origin/main`
  before editing.
- **Implementation commits:** `29afdf8`, `e9af9c1`, `4d78c40`, `7fa5c3c`

## Scope and ownership

This package owns only the backup configuration seam:

- `src/config.rs`: public `BackupConfig`, safe defaults, `AppConfig.backup`,
  strict `BACKUP_*` environment overrides, cloned final validation, endpoint
  allowlisting, and focused `config::tests` selectors.
- `config/agent_voice.example.yaml`: the backup mapping with disabled-safe
  defaults.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-11a-report.md`: this evidence
  report.

No snapshot production, SQLCipher envelope, S3 transport, retention execution,
durable attempt history, health/alerts, restore, CLI, browser/admin surface,
deployment, provider, or live-credential behavior was added.

## RED evidence

The required contract test was added before the production type and run from the
fresh worktree:

```text
rtk cargo test --lib config::tests::backup_config_contract -- --exact --nocapture
```

It exited `101` with `11 errors, 0 warnings`, including the expected missing
`BackupConfig` type and `AppConfig.backup` field. This was a true missing-contract
failure, not a zero-test filtered result.

## GREEN evidence

After the implementation, the five package selectors passed:

```text
rtk cargo test --lib config::tests::backup_config_contract -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_defaults_disabled_and_safe -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_env_overrides_are_strict_and_normalized -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_rejects_destination_escape_and_secrets -- --exact --nocapture
rtk cargo test --lib config::tests::backup_config_snapshot_and_runtime_handoffs -- --exact --nocapture
```

Each selector executed one test and passed (`571 filtered out` in the final
target listing). The complete config module also passed:

```text
rtk cargo test --lib config -- --nocapture
cargo test: 40 passed, 532 filtered out
```

The selectors cover disabled defaults and deterministic equality, all eight
documented overrides with quote/whitespace normalization and atomic rejection,
bucket/region/prefix/endpoint/scratch-path escape checks, redacted diagnostics,
unknown secret fields, production HTTPS versus explicit test-only loopback
endpoint handling, YAML round-trip, and the exact example mapping.

## Validation evidence (LOCAL)

| Check | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 src/config.rs` | PASS |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk git diff --check` | PASS |
| `rtk make check` | PASS — 572 Rust tests, clippy, Rust docs, and Docusaurus build completed successfully |
| `rtk cargo fmt --all -- --check` | BLOCKED by pre-existing formatting drift in untouched `src/pa/fakes/mail.rs`, `src/service.rs`, and `tests/../src/realtime/server_audio_events.rs`; owned `src/config.rs` passed the scoped formatter check |
| `rtk npm ci` in `website/` | PASS setup only; no manifest/lockfile changes; npm reported existing audit/deprecation notices |

The repository-wide formatter warning is not attributed to this package and no
unowned file was changed to mask it.

## Contract mapping

| Contract | Evidence | Status |
| --- | --- | --- |
| `BackupConfig` has the frozen public serde fields and defaults | `backup_config_defaults_disabled_and_safe`, source review | PASS |
| Unknown backup keys are rejected | `backup_config_rejects_destination_escape_and_secrets` | PASS |
| Overrides win over YAML, normalize once, and reject blank/malformed values | `backup_config_env_overrides_are_strict_and_normalized`; `AppConfig::load` handoff | PASS |
| Enabled destinations fail closed on unsafe bucket, region, prefix, endpoint, policy, or scratch path | `backup_config_rejects_destination_escape_and_secrets` | PASS |
| Production endpoint is HTTPS-only; loopback HTTP is test-only | `backup_config_snapshot_and_runtime_handoffs` | PASS |
| Destination/path values never appear in `BackupConfig` Debug or validation errors | redaction assertions and stable field/code errors | PASS |
| Example documents the exact disabled-safe mapping | `backup_config_snapshot_and_runtime_handoffs` | PASS |

## Security, idempotency, and non-claims

Validation is performed on a clone and assigned only after success. A failed
environment parse or validation therefore publishes no partial `AppConfig` and
performs no file, clock, socket, network, provider, database, or token action.
`BackupConfig` has no key, token, credential, or master-key field; unknown
secret-shaped mapping keys are rejected. Re-loading identical values produces
equal typed values. The package does not claim S3, restore, retention deletion,
freshness alert delivery, deployment, OAuth, UAT, or production readiness.

## Lifecycle and delivery

Issue #85 was moved from `status:blocked` to `status:in-progress` after #218 was
confirmed closed, and the pickup comment records the exact base SHA, worktree,
branch, and RED result. The delivering PR must contain exactly `Closes #85` and
`Refs #62`, `Refs #218`, `Refs #107`, `Refs #109`, `Refs #110`, `Refs #111`,
`Refs #112`, `Refs #113`, and `Refs #120`; it must not close any prerequisite,
downstream handoff, or parent tracker. CI and review evidence are not claimed
until a PR reports them.
