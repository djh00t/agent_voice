# Task 5b5 verification report: deterministic encrypted-backup fake

> Package issue: [#188](https://github.com/djh00t/agent_voice/issues/188)<br>
> Related receipt-observability issue: [#199](https://github.com/djh00t/agent_voice/issues/199)<br>
> Feature: [#137](https://github.com/djh00t/agent_voice/issues/137)<br>
> Package ID: `task-5b5`<br>
> Verification date: 2026-08-30 (Australia/Sydney)<br>
> Worktree: `/private/tmp/agent-voice-pa-task5-build`<br>
> Branch: `codex/agent-voice-pa-05-provider-contracts`

## Scope and boundary

This report covers the deterministic encrypted-backup fake and its export
surface. The implementation boundary is:

- `src/pa/fakes/backup.rs`: `FakeEncryptedS3Backup`, backup behavior, and
  focused tests.
- `src/pa/fakes/mod.rs`: export-only module wiring and availability test.
- This report file.

The fake accepts only the typed `EncryptedSnapshot` provider input. It does
not create snapshots, encrypt databases, contact S3, sign requests, read
credentials, or access the network. The current worktree also contains
uncommitted sibling provider-contract changes outside this package; those
changes are not part of this report's ownership.

## Contract checklist

The source contains the following contract coverage. `STATIC` means verified
by reading the owned source and tests. The executable results are recorded in
the local verification table below.

| Requirement | Evidence | Status |
| --- | --- | --- |
| Cloneable fake with shared mutex state and `FakeControl` | `FakeEncryptedS3Backup` derives `Clone`; object state is `Arc<Mutex<BackupState>>`; control is cloned from the supplied `FakeControl` | `STATIC` |
| `BackupPut` begins exactly once before mutation | `put_snapshot` calls `control.begin(FakeOperation::BackupPut)` once and only then calls `store` | `STATIC` |
| Deterministic version, time, key, checksum, and byte-count receipt | `store` uses the monotonic `fake-s3-version-N` sequence, fixed `FakeControl::now()`, and the typed snapshot fields | `STATIC` |
| Exact retry is idempotent | An existing equal snapshot returns its original cloned receipt without a second object | `STATIC` |
| Changed ciphertext/checksum/size/encryption metadata conflicts without mutation | Existing object comparison is full `EncryptedSnapshot` equality; focused regression enumerates each changed value | `STATIC` |
| Failure does not consume objects or provider versions | Control failure occurs before `store`; sequence advances only after receipt construction and insertion | `STATIC` |
| Queued, persistent, and unavailable failure recovery | Focused tests cover token-expired, throttled, unavailable, and persistent failure modes through `FakeControl` | `STATIC` |
| Poisoned state fails closed; replacement recovers | State lock maps poison to `ProviderError::Unavailable`; a fresh fake starts at version 1 | `STATIC` |
| Clone/concurrent exact uploads share one object and receipt | Shared state is mutex-protected; focused `tokio::join!` test checks equal receipts and version 1 | `STATIC` |
| Stable receipt inspection for #199 | `stored_receipts()` locks only backup state, clones receipts from the `BTreeMap`, and returns object-key order without calling `FakeControl` | `STATIC` |
| Poisoned receipt inspection fails closed | `stored_receipts()` maps a poisoned state lock to `Unavailable`; focused regression checks the operation count is unchanged | `STATIC` |
| Debug redaction | `FakeEncryptedS3Backup` debug output contains only `object_count` and `put_call_count`; `BackupReceipt` debug is redacted by the provider contract | `STATIC` |
| No plaintext input/state | The implementation stores `EncryptedSnapshot` and `BackupReceipt` only; no plaintext snapshot type, logging, URL, credentials, or network operation is introduced | `STATIC` |
| Trait object and future safety | `EncryptedS3BackupProvider` is exercised through a trait object; the focused test asserts `FakeEncryptedS3Backup: Send + Sync` and the returned future is `Send` | `STATIC` |

## RED record

No pre-implementation failing run was captured in this worktree. The package
arrived with its implementation and focused regressions already present. Per
the package requirement, this report does not invent a historical RED command
or failure message.

The post-implementation runs below are GREEN evidence only. They do not
retroactively establish a RED run.

## Local verification

All results in this section are **LOCAL** command evidence from the worktree
identified above. No result is promoted to CI or LIVE evidence.

### Focused backup-fake tests

Command:

```text
rtk cargo test --lib pa::fakes::backup::tests --no-fail-fast
```

Result: **PASS** — 14 focused backup-fake tests passed and 422 unrelated
library tests were filtered out.

The focused selector exercises exact retry receipts, receipt observability,
ordering, immutable conflicts, failure recovery, monotonic versions, clone
sharing, concurrency, poisoned state/control recovery, trait-object use,
future `Send`, and debug redaction.

The related fake-family selector was also run:

```text
rtk cargo test --lib pa::fakes --no-fail-fast
```

Result: **PASS** — 109 fake tests passed and 325 unrelated library tests were
filtered out.

### Full library tests

Command:

```text
rtk cargo test --all-targets --no-fail-fast
```

Result: **PASS** — 436 all-target unit tests passed across 2 suites.

### Doctests

Command:

```text
rtk cargo test --doc
```

Result: **PASS** — 3 doctests passed.

### Warning-denied rustdoc

Command:

```text
rtk env RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
```

Result: **PASS** — documentation generated for `agent_voice` with warnings
denied.

### Strict Clippy

Command:

```text
rtk cargo clippy --all-targets --all-features -- -D warnings
```

Result: **PASS** — no Clippy issues found with warnings denied.

### Integrated repository gate

Command:

```text
rtk make check
```

Result: **PASS** — the repository gate completed successfully, including 436
library tests, doctests, strict linting, Rust documentation, and the docs
build.

### Owned-file formatting

Command:

```text
rtk rustfmt --edition 2024 --check src/pa/fakes/backup.rs src/pa/fakes/mod.rs
```

Result: **PASS** — both owned Rust files are formatted.

The repository-wide formatter command was also checked:

```text
rtk cargo fmt -- --check
```

This is not part of the package-scoped gate because it also formats unrelated
working-tree files. The owned files have no trailing-whitespace matches in the
scoped check:

```text
rtk rg -n '[[:blank:]]+$' src/pa/fakes/backup.rs src/pa/fakes/mod.rs
```

The untracked owned implementation file was also checked with the no-index
diff checker so whitespace errors could not be hidden by Git's untracked-file
handling:

```text
rtk git diff --check --no-index /dev/null src/pa/fakes/backup.rs
```

Result: **PASS for whitespace diagnostics** — no whitespace errors were
reported (the no-index comparison itself naturally has a non-zero difference
status because the file is not yet tracked).

### Local evidence summary

| Check | Tier | Result |
| --- | --- | --- |
| Focused backup tests | LOCAL | PASS (14) |
| Fake-family tests | LOCAL | PASS (109) |
| Full library tests | LOCAL | PASS (436) |
| All-target tests | LOCAL | PASS (436 across 2 suites) |
| Doctests | LOCAL | PASS (3) |
| Warning-denied rustdoc | LOCAL | PASS |
| Strict Clippy | LOCAL | PASS |
| `rtk make check` | LOCAL | PASS |
| Owned-file rustfmt | LOCAL | PASS |
| Scoped whitespace diff | LOCAL | PASS |

## Static redaction and security review

- The provider boundary requires `EncryptedSnapshot`; the fake has no API for
  plaintext input and retains only the validated encrypted snapshot plus its
  typed receipt.
- `FakeEncryptedS3Backup`'s manual `Debug` implementation formats counts only.
  It does not format the object map, snapshot, receipt fields, control state,
  object key, checksum, encryption metadata, ciphertext, provider version, or
  session token.
- `stored_receipts()` is a fake-only inspection helper. It reads the shared
  object map without invoking `FakeControl`, clones typed receipts, and relies
  on `BTreeMap` ordering. Receipt debug output is redacted by the provider
  contract; receipt accessors remain available to typed tests for integrity
  assertions.
- `FakeControl::begin` is the only operation-accounting/fault-injection call,
  and it precedes every possible state mutation. A failed or poisoned control
  therefore cannot advance the backup sequence or create an object.
- Mutex poisoning is converted to the closed `Unavailable` error. No recovery
  attempt consumes or rewrites the poisoned state; a new fake is the explicit
  deterministic recovery path.
- No dependency, credential, URL, provider, deployment, logging, or network
  behavior is introduced by the owned files.

## Review, CI, and LIVE boundaries

### Reviewer outcome

No independent reviewer outcome is present in this worktree or on issue #188
at report time. Review is **PENDING**. This report records source-level
findings and actual local command results; it does not self-approve the
package or close the issue.

### CI

**NOT CLAIMED.** Issue #188 has no implementation PR/CI result attached at
report time. The implementation PR must supply a remote check run before CI
can be marked green.

### LIVE

**NOT CLAIMED / OUT OF SCOPE.** This is a deterministic in-memory fake. No S3
account, encrypted database snapshot, OAuth credential, production backup,
deployment, or live provider operation was exercised.

## PR linkage and remaining actions

The implementation PR must contain the exact closing footer:

```text
Closes #188
```

If the same PR carries the #199 `stored_receipts()` implementation, it must
also contain `Closes #199`; otherwise #199 needs its own implementation PR and
report. The current local evidence supports the package's test and quality
claims; CI and LIVE remain explicitly unclaimed below.

Remaining actions:

1. Obtain independent review of `backup.rs` and `fakes/mod.rs`; resolve every
   finding before closing #188.
2. Open the reviewable implementation PR with `Closes #188` (and `Closes
   #199` when #199 is delivered in the same PR), then monitor CI.

**Package status: `status:review` / locally verified.** The implementation,
static contract, focused tests, full local gate, and documentation checks are
green. Independent review, the implementation PR, remote CI, and any live
provider exercise remain outstanding.
