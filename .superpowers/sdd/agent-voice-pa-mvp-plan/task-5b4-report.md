# Task 5b4 report: deterministic structured-triage fake

- **Issue:** #187 (`[Task 5b4] deterministic structured-triage fake`)
- **Feature:** #137, provider contracts and deterministic fakes
- **Package:** `task-5b4`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-task5-build`
- **Branch:** `codex/agent-voice-pa-05-provider-contracts`
- **Reviewed Task 5 stack:** `5ca2aea` (`docs(fakes): refresh provider matrix evidence`)

## Scope and ownership

This package owns only:

- `src/pa/fakes/triage.rs`
- export-only `src/pa/fakes/mod.rs`
- this report

It does not authorize provider calls, credentials, network access, dependency
changes, deployment, commit, push, merge, or production activity. The current
worktree also contains other package files; they were not changed for this
report.

## Contract checked

`FakeStructuredTriage` is cloneable and stores exact validated
`(TriageInput, TriageDecision)` fixtures in an immutable shared map with a
shared `FakeControl`.

- Duplicate source identities are rejected with `Conflict`; `new` panics only
  for that constructor error and `try_new` returns it.
- `classify` begins `TriageClassify` exactly once, returns the exact seeded
  decision, returns `NotFound` for an unknown source, and returns `Conflict`
  when sender, subject, or body differs for a known source.
- Fixtures are never consumed. Exact calls through clones and concurrent calls
  remain stable.
- Queued and persistent token-expired, throttled, and unavailable failures are
  returned without fixture mutation; clearing the control recovers
  deterministically. Poisoned control fails closed and a fresh fake recovers.
- `Debug` exposes fixture/call counts only (or an unavailable marker) and does
  not expose source IDs, sender, subject, body, decision content, token, or due
  data. The fake is `Send + Sync`, the provider trait remains object-safe, and
  its future is `Send`.

## RED evidence

No historical failing-first run was available in this report-only handoff. I do
not claim that a prior RED command was executed.

The implementation and focused tests were already present when this report was
assembled, so the original missing-surface failure cannot be replayed without
discarding accepted work. The expected RED condition was the absent
fake/export surface; this report records that as reconstructed context only.

**RED status:** unavailable; no invented output.

## GREEN evidence (LOCAL)

Commands were run from the worktree above. Output is recorded at LOCAL
evidence tier only.

| Check | Command | Result |
| --- | --- | --- |
| Focused fake tests | `rtk cargo test --lib pa::fakes::triage` | `8 passed, 428 filtered out` |
| Full library tests | `rtk cargo test --lib` | `436 passed` |
| Doctests | `rtk cargo test --doc` | `3 passed` |
| Warning-denied Rust docs | `rtk env RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps` | Finished; docs generated at `target/doc/agent_voice/index.html` |
| Scoped formatting | `rtk rustfmt --edition 2024 --check src/pa/fakes/triage.rs` | Passed |
| Scoped export formatting | `rtk rustfmt --edition 2024 --check src/pa/fakes/mod.rs` | Passed |
| Scoped whitespace check | `rtk git diff --check -- src/pa/fakes/triage.rs src/pa/fakes/mod.rs` | Passed |

The focused suite covers eight tests: all actionable/ambiguous/ignore decision
variants; exact repeat/clone/concurrency behavior; unknown-versus-conflicting
inputs; duplicate rejection; queued/persistent failure recovery; poisoned
control replacement; and debug/trait-object/future redaction and sendability.

For transparency, a whole-worktree `rtk cargo fmt -- --check` was not clean
because it reported unrelated formatting differences in
`src/pa/providers.rs` and `src/service.rs`, outside this package's ownership.
Those files were not changed by this report task. No `make check` claim is made:
the package-specific tests/docs/format/diff commands above are the evidence
actually run.

## Review outcome

Static review of the owned implementation and tests found the contract surface
and redaction boundaries described above. GitHub issue #187 had no comments at
the time of inspection. No external reviewer decision, PR review, or remote
workflow result is available in this report.

## Residual non-claims and handoff

- **CI:** not run or observed here; do not treat LOCAL results as CI evidence.
- **LIVE:** no Microsoft Graph, Gmail, Google Calendar, OpenAI, OAuth, S3,
  network, credential, or production behavior was exercised.
- **RED:** unavailable as stated above; do not backfill a fabricated failure.
- **PR linkage:** the implementation PR must contain `Closes #187` and retain
  the package scope. The issue should remain open until the linked PR is
  reviewed, all findings are resolved, and the PR is merged.
