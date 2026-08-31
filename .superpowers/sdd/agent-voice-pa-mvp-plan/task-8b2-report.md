# Task 8b.2 report: OAuth callback validation and single-use consume

- **Issue:** #250
- **Package:** `task-8b2`
- **Base:** `a20a28be3be37c84cbe5046415497b7053dd8906` (`origin/main`)
- **Branch:** `codex/issue-250`
- **Worktree:** `/Users/djh/work/src/github.com_local/djh00t/agent_voice`
- **Implementation commits:** `f8909586f051b292afbc5cea5487cc41cc2d0a08` (tests), `7d636e769e2ee65328fc1fb61e0a4bb1d21b6c2c` (implementation), `0a5c4c86b29bac6387df300b6fd5aecff08457f6` (warning cleanup)
- **Report commit:** `THIS_REPORT_COMMIT` (resolve with `rtk git rev-parse HEAD` after checkout)

## Scope

This package owns only `src/pa/oauth_callback.rs`,
`tests/oauth_callback_contract.rs`, and this report. The callback boundary
rejects blank codes before state-store access, consumes a reserved state once,
propagates the fixed #249 state errors, and returns an opaque authorization
code whose access is limited to `as_str()`. It performs no token exchange,
HTTP, provider, credential, filesystem, persistence, or prompt/model action.

## Lifecycle pickup

The repository convention for a picked-up work package is the label
transition from `status:blocked` to `status:in-progress`. Issue #250 was
updated with that exact transition and reread as `OPEN` with its original
milestone and `status:in-progress` label. No separate lifecycle comment is
required by the repository convention.

## Evidence records

Each entry records actual package evidence without callback values, state,
verifiers, credentials, tokens, complete query URLs, or provider diagnostics.

```json
[
  {
    "tier": "LOCAL",
    "kind": "RED",
    "selector_or_scope": "callback_rejects_mismatched_state",
    "command_or_check": "rtk cargo test --test oauth_callback_contract callback_rejects_mismatched_state -- --exact",
    "expected": "nonzero missing-contract failure before implementation",
    "exit_code": 101,
    "observed": "failed because tests/../src/pa/oauth_callback.rs was absent; the selector was discovered and did not filter to zero tests",
    "commit": "d3f41c446dacf6cc674ebc3d907c2cf067d3fb5a",
    "timestamp_utc": "2026-08-31T14:38:37Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "callback_rejects_mismatched_state",
    "command_or_check": "rtk cargo test --test oauth_callback_contract callback_rejects_mismatched_state -- --exact",
    "expected": "the exact mismatched-state callback selector passes",
    "exit_code": 0,
    "observed": "1 passed, 28 filtered out",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "oauth_callback_contract",
    "command_or_check": "rtk cargo test --test oauth_callback_contract",
    "expected": "all direct-path callback contract tests pass",
    "exit_code": 0,
    "observed": "29 passed in the direct-path suite, including the seven callback contract cases",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "src/pa/oauth_callback.rs and tests/oauth_callback_contract.rs",
    "command_or_check": "rtk rustfmt --edition 2024 --check src/pa/oauth_callback.rs tests/oauth_callback_contract.rs",
    "expected": "owned files pass scoped formatting",
    "exit_code": 0,
    "observed": "rustfmt check passed with no output",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "working tree diff",
    "command_or_check": "rtk git diff --check",
    "expected": "no whitespace errors",
    "exit_code": 0,
    "observed": "diff check passed with no output",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "repository validation gate",
    "command_or_check": "rtk make check",
    "expected": "Rust tests, strict lint, Rust docs, and website build pass",
    "exit_code": 0,
    "observed": "567 Rust tests ran; lint, Rust docs, and the remaining repository checks completed successfully with no warnings",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:47:40Z"
  },
  {
    "tier": "STATIC",
    "kind": "REVIEW",
    "selector_or_scope": "issue #250 lifecycle metadata",
    "command_or_check": "rtk gh issue edit 250 --repo djh00t/agent_voice --remove-label status:blocked --add-label status:in-progress; rtk gh issue view 250 --repo djh00t/agent_voice --json number,title,state,labels,milestone,url",
    "expected": "picked-up work package is OPEN with status:in-progress and without status:blocked",
    "exit_code": 0,
    "observed": "issue #250 remained OPEN in the agent_voice_pa MVP milestone with status:in-progress and no status:blocked label",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "STATIC",
    "kind": "REVIEW",
    "selector_or_scope": "owned implementation, direct-path tests, issue #250, and #249 handoff",
    "command_or_check": "manual contract review plus scoped immutable diff inspection",
    "expected": "strict validation order, one-time state semantics, opaque code access, redacted diagnostics, and no out-of-scope integration",
    "exit_code": 0,
    "observed": "reviewed exact public signatures and acceptance cases; blank code is checked before consume, consume errors remain fixed and redacted, verifier output is discarded, code/state callback formatters are redacted, and only the three assigned paths changed",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "CI",
    "kind": "NOT_RUN",
    "selector_or_scope": "remote pull-request checks",
    "command_or_check": "not run at report authoring time",
    "expected": "independent CI evidence after PR creation",
    "exit_code": null,
    "observed": "pending remote pull-request evidence; local checks do not imply CI",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  },
  {
    "tier": "LIVE",
    "kind": "NOT_RUN",
    "selector_or_scope": "OAuth consent, provider, network, deployment, and UAT",
    "command_or_check": "not run by package boundary",
    "expected": "no live-provider action",
    "exit_code": null,
    "observed": "explicitly excluded; no credentials, consent, HTTP, token, provider, or deployment operation was performed",
    "commit": "0a5c4c86b29bac6387df300b6fd5aecff08457f6",
    "timestamp_utc": "2026-08-31T14:44:02Z"
  }
]
```

## Acceptance mapping

- `valid_callback_returns_exact_code_and_consumes_state_once` proves the
  exact code is preserved and the reserved state cannot be replayed.
- `callback_rejects_blank_code_before_touching_state_store` proves strict
  validation order and no mutation on blank input.
- The unknown, mismatched, and expired cases prove fixed state errors and
  preserve the valid reservation after failed callbacks.
- The store-failure case proves a valid nonblank callback fails closed when
  the state-store boundary fails.
- `authorization_code_debug_and_display_are_fixed_and_redacted` proves code
  and callback diagnostics do not expose callback values.

## Non-claims and handoff

CI, OAuth consent, credentials, provider/network behavior, deployment, and
authenticated UAT were not run. Task 8b.3 (#251) consumes `OAuthCallback`,
`AuthorizationCode`, and `validate_callback`; the public facade and module
registration remain outside this package.
