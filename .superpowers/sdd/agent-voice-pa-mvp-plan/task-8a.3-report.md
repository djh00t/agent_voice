# Task 8a.3 report: AppConfig OAuth integration

- **Issue:** [#218](https://github.com/djh00t/agent_voice/issues/218)
- **Package:** `task-8a.3`
- **Evidence date:** 2026-08-31 (Australia/Sydney)
- **Worktree:** `/Users/djh/.codex/worktrees/agent_voice-08a3`
- **Branch:** `codex/agent-voice-pa-08a3-oauth-config`
- **Base:** `9daaefec70666f1bd4e35396bd4385136ab45992` (`origin/main`)
- **Implementation commits:** `88f9fd4`, `09b3be4`

## Scope and ownership

This package wires the existing #214/#216 OAuth value types through the
application configuration boundary:

- `src/config.rs` adds the defaulted `agent_api.oauth` field, applies exactly
  four credential environment overrides, normalizes and validates OAuth once
  after all environment overrides, and contains the three focused tests.
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

The fresh worktree had no `website/node_modules`, so `rtk npm ci` was required
for `make check`; it completed without changing either package manifest or
lockfile. npm reported its existing audit/deprecation notices during setup.

## Acceptance mapping

| Contract | Evidence | Status |
| --- | --- | --- |
| Existing YAML without `oauth` retains `agent_api.listen` and safe OAuth defaults | `app_config_oauth_path_defaults_when_omitted` and `#[serde(default)]` | PASS |
| YAML credentials are overridden by the four documented environment keys | `app_config_oauth_env_overrides_yaml_and_blank_fails` and `PaOAuthConfig::apply_env_overrides_from_map` | PASS |
| Present blank environment credentials remain present and fail before consumers | Same focused selector and the single load-path normalization call | PASS |
| Example has only the four credential placeholders and exact provider defaults | `oauth_example_has_only_documented_credential_placeholders` | PASS |
| No secret value is serialized, logged, or sent to a provider | Existing `Secret` redaction contract; no protocol or HTTP changes | PASS |

## Non-claims and handoff

- **CI:** pending the delivering review-ready PR; no CI result is claimed here.
- **LIVE:** no OAuth credentials, provider, HTTP, SIP, deployment, or UAT action
  was run.
- **Side effects:** configuration normalization is in-memory only; no OAuth
  state, socket, client, token, or partial publication is created on failure.
- **Delivery:** the two implementation commits are intentionally separate
  one-file commits; this report is delivered as its own one-file commit.

## Package status

`status:in-progress` at pickup and locally verified, ready for review. The
implementation is limited to the exact three paths named by issue #218.
