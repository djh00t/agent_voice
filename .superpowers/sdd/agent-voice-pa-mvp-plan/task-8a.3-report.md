# Task 8a.3 report: AppConfig OAuth integration

- **Issue:** [#218](https://github.com/djh00t/agent_voice/issues/218)
- **Package:** `task-8a.3`
- **Evidence date:** 2026-09-01 (Australia/Sydney)
- **Lifecycle refresh:** [#315](https://github.com/djh00t/agent_voice/issues/315)
- **Historical worktree:** `/Users/djh/.codex/worktrees/agent_voice-08a3`
- **Historical branch:** `codex/agent-voice-pa-08a3-oauth-config`
- **Historical base:** `9daaefec70666f1bd4e35396bd4385136ab45992` (`origin/main`)
- **Historical implementation commits:** `88f9fd4`, `09b3be4`, `1e10a34`

## Current lifecycle evidence (read-only)

The authoritative GitHub readbacks on 2026-09-01 report the following:

| Object | State and lifecycle evidence |
| --- | --- |
| [#74](https://github.com/djh00t/agent_voice/issues/74) | **OPEN**; this report does not close the parent tracker |
| [#214](https://github.com/djh00t/agent_voice/issues/214) | **CLOSED** by merged [PR #227](https://github.com/djh00t/agent_voice/pull/227), head `feca0d3581eb0615b421da53e0555c9e711b586f`, merge commit `1ff008dadcdda8c753b0559b137eed331c3af7dc` |
| [#216](https://github.com/djh00t/agent_voice/issues/216) | **CLOSED** by merged [PR #234](https://github.com/djh00t/agent_voice/pull/234), head `32d874b5b44559b02d41521cbd10b9d05fd735ab`, merge commit `322e60ce8b0f3b8b277a9a956cf7bd099697b129` |
| [#218](https://github.com/djh00t/agent_voice/issues/218) | **CLOSED/MERGED** by merged [PR #293](https://github.com/djh00t/agent_voice/pull/293), final head `9270b18acbf70313429926c303777f5f29c095f6`, merge commit `9f29bfd539dee0e6fd009dcc27e4eda305c8556f` |

Each final head above passed `rtk git merge-base --is-ancestor <head> origin/main`; the current `origin/main` is `a20a28be3be37c84cbe5046415497b7053dd8906`. The parent tracker remains open pending the post-merge lifecycle audit described in issue #315.

## Lifecycle refresh verification

- **GitHub JSON readbacks:** `gh issue view` confirmed #74 is OPEN and #214,
  #216, and #218 are CLOSED; `gh pr view` confirmed #227, #234, and #293 are
  MERGED with the heads and merge commits recorded above.
- **CI readback:** `gh run view 33383464016` reported workflow `CI`, final head
  `9270b18acbf70313429926c303777f5f29c095f6`, `status: completed`, and
  `conclusion: success`; `gh pr checks 293` reported five passed checks.
- **Final-head ancestry:** `rtk git merge-base --is-ancestor` passed for all
  three final heads against current `origin/main`.
- **Committed-range whitespace:** `rtk git diff --check origin/main...HEAD`
  passed for the report-only range.
- **Stale-claim scan:** the status/open/blocked/ready-for-review selector
  returned only the explicitly labelled historical pickup status below.

## Scope and ownership

This package wires the existing #214/#216 OAuth value types through the
application configuration boundary:

- `src/config.rs` adds the defaulted `agent_api.oauth` field, applies exactly
  four credential environment overrides, normalizes and validates OAuth once
  after all environment overrides, isolates that validation from unrelated
  config fields for security-analysis precision, and contains the three
  focused tests.
- `config/agent_voice.example.yaml` documents the exact Microsoft and Google
  provider defaults with four credential placeholders.
- `.superpowers/sdd/agent-voice-pa-mvp-plan/task-8a.3-report.md` records this
  package's RED/GREEN and review evidence.

No OAuth protocol, HTTP, callbacks, token exchange, persistence, dependencies,
endpoint environment overrides, or live-provider behavior was added.

## Historical RED evidence

The following historical selectors were run after adding the tests but before
the production wiring and example mapping. They are retained as historical
RED evidence and do not describe the current package state:

```text
rtk cargo test --lib config::tests::app_config_oauth_path_defaults_when_omitted -- --exact
rtk cargo test --lib config::tests::app_config_oauth_env_overrides_yaml_and_blank_fails -- --exact
rtk cargo test --lib config::tests::oauth_example_has_only_documented_credential_placeholders -- --exact
```

Each selector exited nonzero at compilation with the expected missing seam:

```text
error[E0609]: no field `oauth` on type `AgentApiConfig`
cargo test: 9 errors, 0 warnings (1 crates)
```

The example selector reached the test after the wiring was added and before
the YAML mapping was added, then failed on the absent Microsoft placeholder:

```text
assertion `left == right` failed
left: None
right: Some("${AGENT_VOICE_PA_MICROSOFT_CLIENT_ID}")
```

## Historical GREEN evidence

After the two production changes and the example mapping, all issue selectors
and the required existing OAuth regression selectors passed. This is retained
as historical GREEN evidence:

```text
rtk cargo test --lib config::tests::app_config_oauth_path_defaults_when_omitted -- --exact
cargo test: 1 passed, 532 filtered out (1 suite, 0.00s)

rtk cargo test --lib config::tests::app_config_oauth_env_overrides_yaml_and_blank_fails -- --exact
cargo test: 1 passed, 532 filtered out (1 suite, 0.00s)

rtk cargo test --lib config::tests::oauth_example_has_only_documented_credential_placeholders -- --exact
cargo test: 1 passed, 532 filtered out (1 suite, 0.00s)

rtk cargo test --lib config::tests::oauth_deserialization_uses_defaults -- --exact
cargo test: 1 passed, 532 filtered out (1 suite, 0.00s)

rtk cargo test --lib config::tests::oauth_urls_and_credentials_validate_before_use -- --exact
cargo test: 1 passed, 532 filtered out (1 suite, 0.00s)
```

## Historical validation evidence (LOCAL)

| Check | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 --check src/config.rs` | PASS |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk git diff --check` | PASS |
| `rtk make check` | PASS — full Rust suite ran 533 tests, Rust docs built, lint passed, and the website build completed with exit 0 |
| `rtk cargo fmt --all -- --check` | BLOCKED by pre-existing formatting drift in untouched `src/pa/fakes/mail.rs` and `src/service.rs`; owned `src/config.rs` passes the scoped check above |
| PR #293 CI run `33383464016` at final head `9270b18acbf70313429926c303777f5f29c095f6` | PASS — `Analyze (javascript-typescript)`, `Analyze (rust)`, `CodeQL`, `Compose Config`, and `Quality Gates` all passed |

The fresh worktree had no `website/node_modules`, so `rtk npm ci` was required
for `make check`; it completed without changing either package manifest or
lockfile. npm reported its existing audit/deprecation notices during setup.

## Historical CI and review follow-up

The initial PR head `a9c5c72` produced CodeQL alert [#47](https://github.com/djh00t/agent_voice/security/code-scanning/47),
`rust/cleartext-transmission`, at untouched `src/accounting.rs:219`. The
reported source was the required OAuth validation call at
`src/config.rs:48`; the analyzer conflated the mutable `AppConfig` receiver's
OAuth fields with the independent accounting pricing URL. The PR diff never
changed `src/accounting.rs`, and no OAuth credential or URL was transmitted.

The bounded fix in historical intermediate head `1e10a34` validates a cloned
OAuth value and assigns it back only after success. Focused tests, clippy,
scoped rustfmt, and the committed range diff check remained green. The
historical rerun on that intermediate head cleared alert #47; the final
merged lifecycle and current five-check evidence are recorded above.

The automated review's P1 comment incorrectly claimed the three files were in
one commit. The remote commit list confirmed one file per commit (`88f9fd4`,
`09b3be4`, `a9c5c72`, and `1e10a34`), so I replied with the evidence and
resolved that false-positive thread. No code change was made for that review.

## Acceptance mapping

| Contract | Evidence | Status |
| --- | --- | --- |
| Existing YAML without `oauth` retains `agent_api.listen` and safe OAuth defaults | `app_config_oauth_path_defaults_when_omitted` and `#[serde(default)]` | PASS |
| YAML credentials are overridden by the four documented environment keys | `app_config_oauth_env_overrides_yaml_and_blank_fails` and `PaOAuthConfig::apply_env_overrides_from_map` | PASS |
| Present blank environment credentials remain present and fail before consumers | Same focused selector and the single load-path normalization call | PASS |
| Example has only the four credential placeholders and exact provider defaults | `oauth_example_has_only_documented_credential_placeholders` | PASS |
| No secret value is serialized, logged, or sent to a provider | Existing `Secret` redaction contract; no protocol or HTTP changes | PASS |

## Non-claims and handoff

- **CI:** [PR #293](https://github.com/djh00t/agent_voice/pull/293) is merged at
  final head `9270b18acbf70313429926c303777f5f29c095f6`; CI run `33383464016`
  completed successfully and the five checks are recorded above.
- **LIVE:** **NOT RUN** — no OAuth credentials, provider, HTTP, SIP, deployment,
  or UAT action was run.
- **Side effects:** configuration normalization is in-memory only; no OAuth
  state, socket, client, token, or partial publication is created on failure.
- **Delivery:** the three implementation commits are intentionally separate
  one-file commits; this report is delivered as its own one-file commit.

## Package status

Historical pickup status was `status:in-progress`; the current package state is
**CLOSED/MERGED** because #218 is closed by merged PR #293. The implementation
remains limited to the exact three paths named by issue #218.
