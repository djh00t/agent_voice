# Task 6e1 report: opaque owner verification and owner-task preparation

- **Issue:** [#212](https://github.com/djh00t/agent_voice/issues/212)
- **Parent:** [#182](https://github.com/djh00t/agent_voice/issues/182)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Prerequisite:** merged [#210](https://github.com/djh00t/agent_voice/issues/210)
- **Package:** `task-6e1`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-06e1-owner-prepare`
- **Branch:** `codex/agent-voice-pa-06e1-owner-prepare`
- **Base at implementation:** `a559829` (`origin/main` before the later main-line rebase)
- **Implementation commit:** `fd5b501`

## Scope

This package owns the capability/prepare path in `src/pa/service.rs`. It adds
the opaque `OwnerConfirmation` and `OwnerVerified` capabilities, the
provider-free `PreparedOwnerTask` projection, and
`PaService::prepare_owner_task`.

`OwnerConfirmation` can only be minted through
`OwnerConfirmation::from_explicit_yes`; capability fields are private and no
capability or result derives `Serialize`/`Deserialize`. Owner numbers are
normalized through `crate::phonebook::normalize_caller_id`, compared only in
normalized form, and never passed to the store or provider boundary.

`OwnerVerified` binds the normalized caller to a lowercase 64-character
SHA-256 fingerprint of `agent_voice_owner_v1\\0` followed by that caller. It
stores canonical UTC whole-second verification and expiry instants and uses a
half-open 60-second interval (`verified_at <= now < expires_at`). Future,
expired, noncanonical, overflowed, missing, anonymous, malformed, or
nonmatching values fail closed.

`prepare_owner_task` validates the capability and caller before any mutation,
checks canonical start time and checked duration addition, then delegates one
immediate transaction to the existing
`PaStore::save_prepared_owner_task`. The store contract supplies exact
draft/placement retries and immutable conflicts. No calendar/provider method
is called.

The package does not modify stores, migrations, providers, fakes, HTTP/voice
routing, OAuth/Graph adapters, deployment, dependencies, or live-provider
configuration.

## RED evidence

The first focused test was written before the owner capability implementation:

```text
rtk cargo test --lib pa::service::tests::owner_verified_issues_opaque_fingerprint_and_enforces_half_open_window -- --nocapture
```

Observed result:

```text
cargo test: 1 errors, 0 warnings (1 crates)
error[E0432]: unresolved imports `super::OwnerConfirmation`, `super::OwnerVerified`
```

The failure was the intentionally absent owner capability API, not a test
typo or unrelated baseline error.

## Acceptance evidence (LOCAL)

| Contract | Evidence | Result |
| --- | --- | --- |
| Explicit confirmation required | `owner_confirmation_rejects_non_affirmative_and_owner_verification_rejects_bad_binding` | PASS |
| Normalized caller binding and malformed/anonymous rejection | Same focused capability test; `OwnerVerified::issue` and `validate_at` | PASS |
| Exact fingerprint and redacted capability/result output | `owner_verified_issues_opaque_fingerprint_and_enforces_half_open_window`; prepared-result assertions | PASS |
| Half-open 60-second and future boundaries | `owner_verified_issues_opaque_fingerprint_and_enforces_half_open_window` | PASS |
| One atomic draft plus placement and no provider calls | `prepare_owner_task_persists_one_atomic_redacted_provider_free_aggregate` | PASS |
| Validation before writes | `prepare_owner_task_rejects_invalid_capability_and_input_before_any_write` | PASS |
| Exact retry and every immutable conflict | `prepare_owner_task_retries_exactly_and_rejects_every_immutable_conflict` | PASS |
| Atomic rollback after placement failure | `prepare_owner_task_rolls_back_draft_when_placement_write_fails` | PASS |
| Legacy fingerprint fails closed | `prepare_owner_task_rejects_legacy_fingerprint_without_overwriting_it` | PASS |
| Concurrent identical prepares converge | `concurrent_identical_owner_prepares_converge_to_one_stable_aggregate` | PASS |

## GREEN evidence (LOCAL)

| Check | Result |
| --- | --- |
| `rtk cargo test --lib pa::service::tests::owner_verified -- --nocapture` | PASS — 1 passed |
| `rtk cargo test --lib pa::service::tests::prepare_owner_task -- --nocapture` | PASS — 5 passed |
| `rtk cargo test --lib pa::store::tests::owner_task -- --nocapture` | PASS — 9 passed |
| `rtk cargo test --lib pa::service::tests -- --nocapture` | PASS — 51 passed |
| `rtk cargo test --lib -- --nocapture` | PASS — 505 passed |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no reported errors |
| `rtk rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| `rtk cargo doc --no-deps --all-features` | PASS — no reported errors |
| `rtk git diff --check` | PASS |

The focused owner tests also exercise the existing fake controls and assert
zero calendar operations. The race test uses two file-backed `PaStore`
connections and verifies one durable draft and one durable placement after
both service calls return the same result.

## Security and failure review

- Raw configured/presented caller numbers never enter the store call,
  fingerprint output, `Debug`, or error text.
- Fingerprint and result fields are private; ordinary `Debug` is a fixed
  redacted category string.
- Capability, caller, timestamp, timezone, operation-key, and checked-end
  validation precede `save_prepared_owner_task`.
- The existing store immediate transaction creates/retries the draft and
  placement together; the injected placement-failure test confirms no draft
  orphan remains.
- A legacy stored fingerprint cannot equal the current prefixed SHA-256
  capability and is returned as a redacted conflict.
- No Outlook, Google, SIP, OAuth, network, or live-provider behavior was
  exercised by this package.

## Non-claims and residual gates

- Evidence is LOCAL only. This report does not claim CI, peer review,
  deployment, publication, authenticated UAT, or live-provider evidence.
- The later owner-task submission/provider package remains responsible for
  consuming this projection and performing any external calendar mutation.
- Rollback is a code revert of `fd5b501`; no database deletion or remote
  provider cleanup is inferred or attempted.

## Completion evidence

- **Implementer:** delegated service lane for #212
- **Implementation commit:** `fd5b501`
- **Report commit:** added separately after implementation
- **PR/push:** not created or pushed, per task instruction
- **Reviewer:** pending independent review
- **Residual LIVE gates:** CI, review, deployment, authenticated UAT, and
  live-provider verification
