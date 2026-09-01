# Task 6e1 report: opaque owner verification and owner-task preparation

- **Issue:** [#212](https://github.com/djh00t/agent_voice/issues/212)
- **Parent:** [#182](https://github.com/djh00t/agent_voice/issues/182)
- **Feature:** [#138](https://github.com/djh00t/agent_voice/issues/138)
- **Prerequisite:** merged [#210](https://github.com/djh00t/agent_voice/issues/210)
- **Package:** `task-6e1`
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-issue-323`
- **Branch:** `codex/issue-323`
- **Current base:** `a20a28be3be37c84cbe5046415497b7053dd8906` (`origin/main` and GitHub `main`)
- **Implementation commit:** `260e501` (historical PR #248 implementation commit)
- **PR #248 final head:** `f47abfc0eaeb8254082f4b9c2ad9e87fc26d92dd`
- **PR #248 merge commit:** `ef12c79479e9b3d841fcaeec7e8f7d8f2de248cb`

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

## Historical RED evidence (LOCAL implementation history)

The following RED observation was captured during the original #212
implementation, before PR #248 merged. It is retained as historical evidence
and was not rerun for this documentation-only reconciliation.

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

## Historical GREEN evidence (LOCAL implementation history)

The following results were captured during the original #212 implementation
before PR #248 merged. They are preserved as historical LOCAL evidence only;
this report-only reconciliation does not reinterpret them as current CI or
live-provider evidence.

| Check | Result |
| --- | --- |
| `rtk cargo test --lib pa::service::tests::owner_verified -- --nocapture` | PASS — 1 passed |
| `rtk cargo test --lib pa::service::tests::prepare_owner_task -- --nocapture` | PASS — 5 passed |
| `rtk cargo test --lib pa::store::tests::owner_task -- --nocapture` | PASS — 9 passed |
| `rtk cargo test --lib pa::service::tests -- --nocapture` | PASS — 51 passed |
| `rtk cargo test --lib` | PASS — 515 passed |
| `rtk cargo test --all-targets` | PASS — 515 passed (2 suites) |
| `rtk make docs-install && rtk make check` (controller, head `a627b94`) | PASS — 515 library tests, 3 compile-fail doctests, strict Clippy, Rust docs, and Docusaurus |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no reported errors |
| `rtk rustfmt --edition 2024 --check src/pa/service.rs` | PASS |
| `rtk cargo doc --no-deps --all-features` | PASS — no reported errors |
| `rtk git diff --check origin/main...HEAD` | PASS |

The focused owner tests also exercise the existing fake controls and assert
zero calendar operations. The race test uses two file-backed `PaStore`
connections and verifies one durable draft and one durable placement after
both service calls return the same result.

The controller's `npm ci` docs setup reported 24 existing advisories (7
moderate, 17 high). This package makes no dependency changes.

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

## Current GitHub/main and CI evidence

GitHub API readback and the local remote-tracking ref both resolve `main` to
`a20a28be3be37c84cbe5046415497b7053dd8906`. Local ancestry verification
confirms that PR #248's merge commit `ef12c79479e9b3d841fcaeec7e8f7d8f2de248cb`
is an ancestor of that current `origin/main`. GitHub records PR #248 as
`MERGED` at `2026-08-31T09:01:15Z`, with final PR head
`f47abfc0eaeb8254082f4b9c2ad9e87fc26d92dd` and merge commit
`ef12c79479e9b3d841fcaeec7e8f7d8f2de248cb`.

Current checks for main commit `a20a28be3be37c84cbe5046415497b7053dd8906`
are:

| Workflow / run | Observed result |
| --- | --- |
| [CI run 33397442809](https://github.com/djh00t/agent_voice/actions/runs/33397442809) | PASS — Quality Gates and Compose Config |
| [CodeQL run 33397442727](https://github.com/djh00t/agent_voice/actions/runs/33397442727) | PASS — Rust and JavaScript/TypeScript analysis |
| [Container run 33397442646](https://github.com/djh00t/agent_voice/actions/runs/33397442646) | FAIL — Publish Image passed; Deploy tv04 failed during the health check |

The Container failure log reports `agent_api.oauth.microsoft.client_id must
not be blank` after the runtime started, so this report makes no successful
deployment claim. Current main is therefore not an all-green workflow set.

## Non-claims and residual gates

- Historical implementation evidence is LOCAL/controller plus independent
  final review. Current GitHub/main workflow results are recorded above. This
  report does not claim a successful deployment, authenticated UAT, or
  live-provider evidence.
- Independent final review: SPEC PASS and QUALITY PASS, with no residual P0-P3
  findings.
- The later owner-task submission/provider package was delivered by merged PR
  [#294](https://github.com/djh00t/agent_voice/pull/294); this report remains
  preparation-only and does not claim that package's provider-side evidence.
- Rollback is a code revert of `260e501`; no database deletion or remote
  provider cleanup is inferred or attempted.

## Completion evidence

- **Implementer:** delegated service lane for #212
- **Implementation commit:** `260e501`
- **PR #248:** merged at `ef12c79`; issue #212 is CLOSED.
- **Report update:** this file is the sole owned path for the #323
  reconciliation; its delivery PR closes #323 and references #182, #212, and
  #248.
- **Reviewer:** independent final review complete — SPEC PASS, QUALITY PASS,
  no residual P0-P3 findings
- **Residual gates:** deployment, authenticated UAT, and live-provider
  verification remain unproven; current main's tv04 deployment check failed.
