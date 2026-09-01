# Task 8b.3 report: OAuth public facade and module registration

- **Issue:** #251
- **Package:** `task-8b3`
- **Immutable base:** `683249e59f8b302bbf8dc7a1ba9a1f0ab1d076d4` (`origin/main`)
- **Branch:** `codex/issue-251-oauth-facade`
- **Worktree:** `/Users/djh/.codex/worktrees/agent_voice-issue-251-oauth-facade`
- **Historical #251 package range (pre-remediation):** `5d7cfba99f8d61a4e01cb05aa7f67c845dc07019`, `758ef7f7e263e472e9da6ef74bded9cf85cbc363`, `1501ea6ae959e9065adff2a32d19fb372c3cee71`, `a13383c504902a9380ba67c0b687250512c9c1fd`, `752960f874c333d6083a7bc6e2df46ff72985d5c`
- **Final remediated PR range:** #354 commits `4bd2340dac9b6499f19950824ab34f42fb886745` and `e7a4d970a38c045abcd5aab634914c670bb6d490`; #355 report correction `0ff5c4595d82a88cdadfc90965ed0b7ee4a7984d`; #360 is the report-only correction in this file.
- **Pre-export RED working tree:** uncommitted state derived from the immutable base after adding the module-surface test and only `pub mod oauth;` registration; no facade re-exports or child-module registrations existed in that state, so it has no commit SHA.

## Historical #251 package scope (pre-remediation)

This package owns only `src/pa/oauth.rs`, the OAuth registration hunk in
`src/pa/mod.rs`, and this report. The facade re-exports the exact reviewed
#249 and #250 symbols through `crate::pa::oauth`; the child implementation
modules are registered privately exactly once. The module-surface test reaches
the typed start, callback, state-store, and opaque authorization-code API
without executing provider, HTTP, filesystem, credential, or prompt behavior.

Within this historical package range, no child implementation, child test,
provider/client/config code, dependency, OAuth value, URL, credential, or
live-provider path was changed. This historical three-path statement does not
describe the final remediated PR.

## Final remediated PR scope

The final PR range includes the historical #251 paths plus every remediation
owned path: `src/pa/oauth_callback.rs` for the #354 item-level verifier lint
allowance, the #354 removal hunk in `src/pa/mod.rs`, and this report for the
#355 RED evidence correction and the #360 scope correction. The final PR
therefore has four unique changed paths: `src/pa/oauth.rs`, `src/pa/mod.rs`,
`src/pa/oauth_callback.rs`, and this report. It
therefore includes a child implementation-file change in
`src/pa/oauth_callback.rs`; the historical no-child-change statement above is
not a final-range claim.

## Lifecycle pickup

Issue #251 was reread as `OPEN` in the `agent_voice_pa MVP` milestone with
`status:in-progress` and without `status:blocked`. Dependencies #249 and #250
were reread as their reviewed `CLOSED` issues in the same milestone. No parent
tracker or dependency labels were changed.

## Evidence records

Each entry records observed output without secrets, credentials, tokens,
complete query URLs, or provider diagnostics.

The records below are historical #251 package evidence from the
pre-remediation range. Their three-path scope assertions are not claims about
the final remediated PR range stated above.

```json
[
  {
    "tier": "LOCAL",
    "kind": "RED",
    "selector_or_scope": "pa::oauth::tests::module_surface",
    "command_or_check": "rtk cargo test --lib pa::oauth::tests::module_surface -- --exact",
    "expected": "nonzero E0405/E0425/E0433 missing-facade compile failure after pa::oauth registration but before facade re-exports",
    "exit_code": 101,
    "observed": "the discovered selector failed with 16 E0405/E0425/E0433 diagnostics for unresolved pa::oauth start, callback, state-store, and opaque-code symbols; it did not filter to zero tests",
    "commit": "683249e59f8b302bbf8dc7a1ba9a1f0ab1d076d4",
    "working_tree": "uncommitted pre-export state described in package metadata; the immutable base itself contained neither the registration nor an executable selector",
    "timestamp_utc": "2026-09-01T01:39:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "pa::oauth::tests::module_surface",
    "command_or_check": "rtk cargo test --lib pa::oauth::tests::module_surface -- --exact",
    "expected": "the exact module-surface selector passes with a discovered test",
    "exit_code": 0,
    "observed": "1 passed, 579 filtered out",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:44:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "pa::oauth",
    "command_or_check": "rtk cargo test --lib pa::oauth",
    "expected": "all facade library tests pass",
    "exit_code": 0,
    "observed": "1 passed, 579 filtered out",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:44:00Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "src/pa/oauth.rs",
    "command_or_check": "rtk rustfmt --edition 2024 --check src/pa/oauth.rs",
    "expected": "facade source passes scoped formatting",
    "exit_code": 0,
    "observed": "rustfmt check passed with no output",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:47:49Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "src/pa/mod.rs registration hunk",
    "command_or_check": "rtk rustfmt --edition 2024 --check --config skip_children=true src/pa/mod.rs",
    "expected": "registration source passes without traversing unrelated child modules",
    "exit_code": 0,
    "observed": "rustfmt check passed with no output",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:47:49Z"
  },
  {
    "tier": "LOCAL",
    "kind": "REVIEW",
    "selector_or_scope": "requested two-file rustfmt command",
    "command_or_check": "rtk rustfmt --edition 2024 --check src/pa/oauth.rs src/pa/mod.rs",
    "expected": "the requested files pass formatting",
    "exit_code": 1,
    "observed": "rustfmt 1.9 recursively reported four pre-existing formatting diffs in unowned src/pa/fakes/mail.rs; the owned files pass their scoped checks and no unrelated file remains changed",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:48:10Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "working tree diff",
    "command_or_check": "rtk git diff --check",
    "expected": "no whitespace errors",
    "exit_code": 0,
    "observed": "diff check passed with no output",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:47:49Z"
  },
  {
    "tier": "LOCAL",
    "kind": "GREEN",
    "selector_or_scope": "repository validation gate",
    "command_or_check": "rtk make check",
    "expected": "Rust tests, strict Clippy, Rust docs, and Docusaurus build pass",
    "exit_code": 0,
    "observed": "580 Rust tests passed; strict Clippy, Rust docs, and Docusaurus build completed successfully after repository-pinned website dependencies were installed",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:48:20Z"
  },
  {
    "tier": "STATIC",
    "kind": "REVIEW",
    "selector_or_scope": "src/pa/oauth.rs, src/pa/mod.rs, and issue #251 ownership",
    "command_or_check": "manual exact-surface review plus scoped immutable diff inspection",
    "expected": "exact public symbols, exactly-once private child registration, no aliases/catchalls, no behavior or out-of-scope changes",
    "exit_code": 0,
    "observed": "the facade exposes only the specified start/state-store/callback/opaque-code symbols; private child modules are each declared once; only the three owned paths changed",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:48:35Z"
  },
  {
    "tier": "CI",
    "kind": "NOT_RUN",
    "selector_or_scope": "remote pull-request checks",
    "command_or_check": "not run before PR creation",
    "expected": "independent CI evidence after the PR is opened",
    "exit_code": null,
    "observed": "not yet available; local checks do not imply CI",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:48:35Z"
  },
  {
    "tier": "LIVE",
    "kind": "NOT_RUN",
    "selector_or_scope": "OAuth consent, credentials, provider/network, deployment, and UAT",
    "command_or_check": "not run by package boundary",
    "expected": "no live-provider action",
    "exit_code": null,
    "observed": "explicitly excluded; no credentials, consent, HTTP, provider, deployment, or UAT operation was performed",
    "commit": "a13383c504902a9380ba67c0b687250512c9c1fd",
    "timestamp_utc": "2026-09-01T01:48:35Z"
  }
]
```

The initial selector run before `pa::oauth` registration returned 0 passed and
579 filtered tests; it was discarded as invalid zero-test evidence under the
issue contract. The valid RED above was captured in the explicitly named
uncommitted pre-export working tree after registration existed but before
facade exports were added. The immutable base predates that registration and
did not contain an executable selector.

## Acceptance mapping

- The module-surface selector proves callers can reach `begin`,
  `validate_callback`, `OAuthStart`, `OAuthCallback`,
  `AuthorizationCode::as_str`, `InMemoryOAuthStateStore`, and the associated
  typed error/result and traits through `pa::oauth`.
- Private `oauth_start` and `oauth_callback` declarations compile the reviewed
  child files exactly once; no alternate public implementation path is added.
- The facade performs no OAuth protocol, PKCE, callback, HTTP, credential,
  persistence, prompt/model, provider, or deployment behavior.

## Non-claims and handoff

The package does not claim OAuth consent, credentials, provider/network access,
deployment, authenticated UAT, production readiness, merge, or approval. CI
remains a separate delivering-PR claim. The delivering PR footer is
`Closes #251`, `Closes #354`, `Closes #355`, and `Closes #360`, with `Refs #75`,
`Refs #59`, `Refs #249`, `Refs #250`, `Refs #251`, `Refs #354`, `Refs #355`,
and `Refs #353`; it must not close #75, #74, or #59.
