# Task 8b.1 report: OAuth start URL, state, and PKCE core

- **Issue:** #249
- **Package:** `task-8b1`
- **Base:** `f8319aceec157d974b52dca73e192b34653f25d1` (`origin/main`)
- **Branch:** `codex/agent-voice-issue-249`
- **Worktree:** `/private/tmp/agent-voice-issue-249`
- **Final head:** `05a6bcd034f3b66b8bb13fd8930ae648ca565746`
- **Implementation commits:** `7a3ea93df67a7467e86827fe504f2b026c704eb9`, `742cab06cde9878b3a5959976d73337e9322dddc`, `4c36a8260e3340eb8267b63023cf3475fe8671aa`, `1218148d536d8190612720cde3d3e5363cc41901`, `791acde3d26bba8c1b15d3522fde1c4e527bb88f`, `05a6bcd034f3b66b8bb13fd8930ae648ca565746`
- **Report commit:** `65cb6f7360938b63f461a445861b3482512b2aa9`

## Scope

This package owns only `src/pa/oauth_start.rs`,
`tests/oauth_start_contract.rs`, and this report. It provides the typed
authorization-start boundary, RFC 7636 S256 state/verifier generation,
provider-specific query assembly, and deterministic in-memory single-use state
storage. It performs no HTTP, provider, consent, token, persistence, or
credential operation and adds no dependency.

## Evidence records

Each entry records the actual check without sensitive values or complete URLs.

```json
[
  {
    "tier": "LOCAL",
    "kind": "RED",
    "selector_or_scope": "pkce_url_contract",
    "command_or_check": "rtk cargo test --test oauth_start_contract pkce_url_contract -- --exact",
    "expected": "nonzero missing-contract failure before implementation",
    "exit_code": 101,
    "observed": "failed because tests/../src/pa/oauth_start.rs was absent; no test was filtered to zero",
    "commit": "2e33636aeba988513ce883b932f7815e1982deb5",
    "timestamp_utc": "2026-08-31T11:24:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "pkce_url_contract",
    "command_or_check": "rtk cargo test --test oauth_start_contract pkce_url_contract -- --exact",
    "expected": "the exact PKCE and provider URL selector passes",
    "exit_code": 0,
    "observed": "1 passed, 29 filtered out",
    "commit": "4143fdcf6ef62a8c60666772448f32ac01c57ac7",
    "timestamp_utc": "2026-08-31T11:31:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "oauth_start_contract",
    "command_or_check": "rtk cargo test --test oauth_start_contract",
    "expected": "all direct-path OAuth-start contract tests pass",
    "exit_code": 0,
    "observed": "30 passed",
    "commit": "4143fdcf6ef62a8c60666772448f32ac01c57ac7",
    "timestamp_utc": "2026-08-31T11:32:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "src/pa/oauth_start.rs and tests/oauth_start_contract.rs",
    "command_or_check": "rtk rustfmt --edition 2024 --check src/pa/oauth_start.rs tests/oauth_start_contract.rs",
    "expected": "owned files pass scoped formatting",
    "exit_code": 0,
    "observed": "rustfmt check passed with no output",
    "commit": "4143fdcf6ef62a8c60666772448f32ac01c57ac7",
    "timestamp_utc": "2026-08-31T11:39:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "working tree and staged owned files",
    "command_or_check": "rtk git diff --check",
    "expected": "no whitespace errors",
    "exit_code": 0,
    "observed": "diff check passed with no output",
    "commit": "4143fdcf6ef62a8c60666772448f32ac01c57ac7",
    "timestamp_utc": "2026-08-31T11:39:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "origin/main baseline",
    "command_or_check": "rtk cargo test --all-targets",
    "expected": "clean baseline before package work",
    "exit_code": 0,
    "observed": "827 passed across 8 suites",
    "commit": "5469cad6862f69264cb55b159c4443038fa84864",
    "timestamp_utc": "2026-08-31T11:20:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "repository validation gate",
    "command_or_check": "rtk make check",
    "expected": "Rust tests, strict Clippy, Rust docs, and Docusaurus build pass",
    "exit_code": 0,
    "observed": "548 library tests passed; lint, Rust docs, and Docusaurus build completed successfully",
    "commit": "b01baa0325722d14a5feb21619f64b3356ac2102",
    "timestamp_utc": "2026-08-31T11:44:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "pkce_url_contract",
    "command_or_check": "rtk cargo test --test oauth_start_contract pkce_url_contract -- --exact",
    "expected": "the exact PKCE and provider URL selector passes after state-store hardening",
    "exit_code": 0,
    "observed": "1 passed, 31 filtered out",
    "commit": "05a6bcd034f3b66b8bb13fd8930ae648ca565746",
    "timestamp_utc": "2026-08-31T11:59:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "oauth_start_contract",
    "command_or_check": "rtk cargo test --test oauth_start_contract",
    "expected": "all direct-path OAuth-start contract tests pass after lifecycle hardening",
    "exit_code": 0,
    "observed": "32 passed",
    "commit": "05a6bcd034f3b66b8bb13fd8930ae648ca565746",
    "timestamp_utc": "2026-08-31T11:59:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "src/pa/oauth_start.rs and tests/oauth_start_contract.rs",
    "command_or_check": "rtk rustfmt --edition 2024 --check src/pa/oauth_start.rs tests/oauth_start_contract.rs",
    "expected": "owned files pass scoped formatting after lifecycle hardening",
    "exit_code": 0,
    "observed": "rustfmt check passed with no output",
    "commit": "05a6bcd034f3b66b8bb13fd8930ae648ca565746",
    "timestamp_utc": "2026-08-31T11:59:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "working tree and staged owned files",
    "command_or_check": "rtk git diff --check",
    "expected": "no whitespace errors after lifecycle hardening",
    "exit_code": 0,
    "observed": "diff check passed with no output",
    "commit": "05a6bcd034f3b66b8bb13fd8930ae648ca565746",
    "timestamp_utc": "2026-08-31T11:59:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "repository validation gate",
    "command_or_check": "rtk make check",
    "expected": "Rust tests, strict Clippy, Rust docs, and Docusaurus build pass after lifecycle hardening",
    "exit_code": 0,
    "observed": "548 library tests passed; lint, Rust docs, and Docusaurus build completed successfully",
    "commit": "05a6bcd034f3b66b8bb13fd8930ae648ca565746",
    "timestamp_utc": "2026-08-31T11:59:00Z"
  },
  {
    "tier": "STATIC",
    "kind": "REVIEW",
    "selector_or_scope": "src/pa/oauth_start.rs, tests/oauth_start_contract.rs, and issue #249 ownership",
    "command_or_check": "manual contract review plus scoped immutable diff inspection",
    "expected": "exact query/schema ownership, atomic failures, bounded state retention, redacted diagnostics, and no network/provider/persistence code",
    "exit_code": 0,
    "observed": "reviewed against #249 and the P2 finding; consumed verifiers are dropped, expiry purge removes replay tombstones, only the three assigned paths changed, and no secret, token, or complete URL is recorded",
    "commit": "05a6bcd034f3b66b8bb13fd8930ae648ca565746",
    "timestamp_utc": "2026-08-31T12:00:00Z"
  },
  {
    "tier": "CI",
    "kind": "NOT_RUN",
    "selector_or_scope": "remote pull-request checks",
    "command_or_check": "not run at report authoring time",
    "expected": "independent CI evidence after PR creation",
    "exit_code": null,
    "observed": "pending remote PR workflow; local checks do not imply CI",
    "commit": "4143fdcf6ef62a8c60666772448f32ac01c57ac7",
    "timestamp_utc": "2026-08-31T11:40:00Z"
  },
  {
    "tier": "LIVE",
    "kind": "NOT_RUN",
    "selector_or_scope": "OAuth consent, provider, network, deployment, and UAT",
    "command_or_check": "not run by package boundary",
    "expected": "no live-provider action",
    "exit_code": null,
    "observed": "explicitly excluded; no credentials, consent, HTTP, token, or provider operation performed",
    "commit": "4143fdcf6ef62a8c60666772448f32ac01c57ac7",
    "timestamp_utc": "2026-08-31T11:40:00Z"
  }
]
```

## Acceptance mapping

- `pkce_url_contract` proves deterministic 32-byte base64url state/verifier,
  ASCII-byte S256 challenge, exact Microsoft/Google query keys, and verifier
  absence from the URL.
- The focused suite proves redirect preservation, pre-query/fragment rejection
  before mutation, checked expiry, duplicate/expired/used state handling,
  bounded replay-tombstone retention, verifier release after consumption,
  failure atomicity, and redacted start/error diagnostics.
- The implementation reads only the normalized client ID, authorization URL,
  redirect URI, and scopes; it does not read `token_url` or `client_secret`.
