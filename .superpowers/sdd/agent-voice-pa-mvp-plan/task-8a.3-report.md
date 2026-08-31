# Task 8a.3 report: AppConfig OAuth integration

- **Issue:** [#218](https://github.com/djh00t/agent_voice/issues/218)
- **Package:** `task-8a.3`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Worktree:** `/Users/djh/.codex/worktrees/agent_voice-08a3`
- **Branch:** `codex/agent-voice-pa-08a3-oauth-config`
- **Base:** `9daaefec70666f1bd4e35396bd4385136ab45992` (`origin/main`)
- **Implementation commits:** `88f9fd4`, `09b3be4`, `1e10a34`

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

## RED evidence

The three issue-mandated selectors were run after adding the tests but before
the production wiring and example mapping:

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

## GREEN evidence

After the two production changes and the example mapping, all issue selectors
and the required existing OAuth regression selectors passed:

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

## Validation evidence (LOCAL)

| Check | Result |
| --- | --- |
| `rtk rustfmt --edition 2024 --check src/config.rs` | PASS |
| `rtk cargo clippy --all-targets --all-features -- -D warnings` | PASS — no issues found |
| `rtk git diff --check` | PASS |
| `rtk make check` | PASS — full Rust suite ran 533 tests, Rust docs built, lint passed, and the website build completed with exit 0 |
| `rtk cargo fmt --all -- --check` | BLOCKED by pre-existing formatting drift in untouched `src/pa/fakes/mail.rs` and `src/service.rs`; owned `src/config.rs` passes the scoped check above |
| PR #293 CI at head `1e10a34` | PASS — Quality Gates, Compose Config, JavaScript CodeQL, Rust CodeQL, and aggregate CodeQL all passed |

The fresh worktree had no `website/node_modules`, so `rtk npm ci` was required
for `make check`; it completed without changing either package manifest or
lockfile. npm reported its existing audit/deprecation notices during setup.

## CI and review follow-up

The initial PR head `a9c5c72` produced CodeQL alert [#47](https://github.com/djh00t/agent_voice/security/code-scanning/47),
`rust/cleartext-transmission`, at untouched `src/accounting.rs:219`. The
reported source was the required OAuth validation call at
`src/config.rs:48`; the analyzer conflated the mutable `AppConfig` receiver's
OAuth fields with the independent accounting pricing URL. The PR diff never
changed `src/accounting.rs`, and no OAuth credential or URL was transmitted.

The bounded fix in `1e10a34` validates a cloned OAuth value and assigns it back
only after success. Focused tests, clippy, scoped rustfmt, and the committed
range diff check remained green. The rerun on head `1e10a34` cleared alert #47;
all five PR checks passed and GitHub reported `mergeStateStatus: CLEAN`.

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

- **CI:** [PR #293](https://github.com/djh00t/agent_voice/pull/293) reports all
  five checks green at head `1e10a34`; mergeability is clean.
- **LIVE:** no OAuth credentials, provider, HTTP, SIP, deployment, or UAT action
  was run.
- **Side effects:** configuration normalization is in-memory only; no OAuth
  state, socket, client, token, or partial publication is created on failure.
- **Delivery:** the three implementation commits are intentionally separate
  one-file commits; this report is delivered as its own one-file commit.

## Package status

`status:in-progress` at pickup and locally verified, ready for review. The
implementation is limited to the exact three paths named by issue #218.
