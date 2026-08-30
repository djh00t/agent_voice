# Task 10b.0 report: Realtime module bootstrap and focused test harness

- **Issue:** [#215](https://github.com/djh00t/agent_voice/issues/215)
- **Package:** `task-10b.0`
- **Evidence date:** 2026-08-30 (Australia/Sydney)
- **Worktree:** `/private/tmp/agent-voice-pa-10b1-realtime-bootstrap`
- **Branch:** `codex/agent-voice-pa-10b1-realtime-bootstrap`
- **Base:** `1ff008dadcdda8c753b0559b137eed331c3af7dc` (`origin/main`)
- **Implementation commits:** `a4a1a15`, `86c4c1d`

## Scope and ownership

This package owns the inert Realtime module boundary and its public crate
registration:

- `src/realtime/mod.rs` contains module documentation and one `#[cfg(test)]`
  module-boundary test.
- `src/lib.rs` contains the adjacent documentation and one public
  `realtime` module declaration.
- No values, child modules, provider access, payload processing, state, or
  dependencies are introduced.

## RED evidence

The issue-mandated selector was run against the clean base before the module
was added:

```text
rtk cargo test --lib realtime::tests::module_boundary_is_inert -- --exact
cargo test: 0 passed, 480 filtered out (1 suite, 0.00s)
```

This is the contract's test-absence RED condition: the `realtime` module and
its test were not registered on `origin/main`.

## GREEN evidence

After adding the test-first module file and the public registration, the same
selector passed:

```text
rtk cargo test --lib realtime::tests::module_boundary_is_inert -- --exact
cargo test: 1 passed, 480 filtered out (1 suite, 0.00s)
```

The test is an empty, side-effect-free boundary probe. The source contains no
provider values, child module declaration, input, I/O, or legacy integration.

## Validation evidence (LOCAL)

| Check | Result |
| --- | --- |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk rustfmt --edition 2024 --check --config skip_children=true src/lib.rs` | PASS |
| `rtk rustfmt --edition 2024 --check src/realtime/mod.rs` | PASS |
| `rtk git diff --check 1ff008d..HEAD` | PASS — committed package range checked |
| `rtk make check` | PASS — Rust tests, lint, docs, and website build completed with exit 0 |

The first `rtk make check` attempt reached the Rust checks but could not run
the website build because this fresh worktree had no `website/node_modules`
(`docusaurus: command not found`). Running `rtk npm ci` from the checked-in
`website/package-lock.json` resolved that setup condition without changing
any dependency manifest or lockfile; the rerun passed.

The requested whole-crate `rtk cargo fmt -- --check` reports unrelated
pre-existing formatting differences in `src/pa/fakes/calendar.rs`,
`src/pa/fakes/mail.rs`, and `src/service.rs`. Those files are outside this
package and were not changed. The owned files pass the scoped rustfmt checks
above.

## Acceptance mapping

| Contract | Evidence | Status |
| --- | --- | --- |
| Crate exposes one public `realtime` module | `src/lib.rs` adjacent docs/declaration | PASS |
| Boundary loads without side effects | `realtime::tests::module_boundary_is_inert` | PASS |
| No event/value or child module declarations | `src/realtime/mod.rs` contains docs and the cfg test only | PASS |
| No dependencies, schemas, provider access, parser dispatch, config, or deployment | Diff and clippy review | PASS |

## Non-claims and handoff

- **CI:** required on [PR #228](https://github.com/djh00t/agent_voice/pull/228);
  CI results remain separate from the LOCAL evidence recorded above.
- **LIVE:** no provider, network, credential, SIP, or deployment behavior was
  exercised.
- **Delivery:** the atomic commits are published for review in PR #228; merge
  and deployment remain unexecuted.
- **Follow-on:** downstream value work may add child declarations in later
  packages; this bootstrap remains inert.

## Package status

`status:review` / locally verified. The implementation is limited to the
three files named by issue #215 and uses separate one-file commits.
