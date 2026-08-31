# Task 8c.3 report: encrypted OAuth token update and atomic persistence

- **Issue:** #254
- **Package:** 'task-8c3'
- **Base:** 'a20a28be3be37c84cbe5046415497b7053dd8906' ('origin/main')
- **Branch:** 'codex/issue-254'
- **Worktree:** '/private/tmp/agent-voice-issue-254'
- **Implementation commits:** 'df736ebe6f827831ff5185d37a1feba0dc8c6ca5',
  'bd0302cff42ff026d5280515fcdf64635e5cfa60'

## Scope and ownership

This package owns only:

- 'src/pa/store.rs': 'PaStore::update_oauth_tokens' and its named method
  hunk.
- 'tests/oauth_token_persistence_contract.rs': store update contract tests.
- This report.

The implementation validates provider, account, access token, optional refresh
token, and canonical scopes before opening an immediate SQLite transaction. It
encrypts token values with the existing 'TokenCipher' and exact access or
refresh AAD, then performs one provider/account upsert. An omitted refresh
value carries forward the existing encrypted envelope, while a supplied value
replaces it. No endpoint request, OAuth state transition, schema migration,
provider call, dependency, or live credential operation was added.

## RED/GREEN evidence

The store test was added before the production method. The required exact RED
selector exited nonzero because the method did not exist:

~~~
rtk cargo test --test oauth_token_persistence_contract omitted_refresh_token_preserves_prior -- --exact
exit_code=101
observed: two E0599 errors for the missing PaStore::update_oauth_tokens method
~~~

After the method was implemented, the same selector passed:

~~~
rtk cargo test --test oauth_token_persistence_contract omitted_refresh_token_preserves_prior -- --exact
exit_code=0
observed: 1 passed, 6 filtered out
~~~

The complete owned contract suite passed:

~~~
rtk cargo test --test oauth_token_persistence_contract
exit_code=0
observed: 7 passed
~~~

The suite covers initial insert with and without a refresh token, explicit
refresh replacement, omitted-refresh preservation, scope normalization,
provider/account isolation, one-row upsert, access/refresh AAD separation,
encrypted-at-rest values, validation immutability, SQL-trigger rollback, and
redacted debug/error output. Assertions avoid printing token values on failure.

## Validation evidence

| Check | Result |
| --- | --- |
| 'rtk rustfmt --edition 2024 --check src/pa/store.rs tests/oauth_token_persistence_contract.rs' | PASS |
| 'rtk git diff --check' | PASS |
| 'rtk make check' | BLOCKED at existing docs-build prerequisite: Rust tests, Clippy, and Rust docs completed, then 'website' reported 'docusaurus: command not found' and make exited 2 |

The first gate run also identified Clippy's unavoidable
'too_many_arguments' lint on the issue-mandated API; the method has a narrow
method-level allow, and the rerun reported no Rust lint errors before the
missing Docusaurus executable stopped the gate. No repository file was changed
to bypass or weaken the docs gate.

## Static contract review

- Validation happens before transaction work and returns fixed field-only
  errors.
- Token envelopes are serialized only after encryption; plaintext token values
  are not inserted into SQLite.
- Access and refresh values use 'oauth:{provider}:{account_id}:access' and
  'oauth:{provider}:{account_id}:refresh' respectively.
- Omitted refresh updates preserve the prior BLOB through the conflict-update
  branch and never replace an existing envelope with SQL NULL.
- The immediate transaction covers encryption, existing-envelope lookup, the
  upsert, and commit; SQL failure drops the transaction and rolls back.
- Provider and account values reject AAD-colliding colons through the existing
  identity validator.
- The implementation does not log, format, display, or include secret values
  in errors. Existing credential and envelope debug redaction remains in use.
- Only the three issue-owned paths are changed.

## Non-claims and handoff

- **CI:** not run at report authoring time; local checks do not imply remote CI.
- **LIVE:** OAuth consent, provider/network calls, credentials, deployment, and
  UAT were not run.
- **Docs gate:** 'make check' needs the repository's website dependencies
  installed before 'docs-build' can execute; this package did not change
  website files or dependencies.
- **Delivery:** implementation, tests, and this report are separate one-file
  commits. The branch is ready to push and open for review, with 'Closes
  #254' and 'Refs #76', 'Refs #59', 'Refs #141', 'Refs #142', and
  'Refs #216' in the PR body.
