# Contributing

## Before You Start

- Read [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
- Read [SECURITY.md](SECURITY.md) before reporting sensitive issues.
- Prefer opening an issue before large changes.

## Development Flow

1. Create a branch for your change.
2. Keep changes small and reviewable.
3. Run the repo checks before pushing:

```bash
make check
```

4. Update documentation when behavior or configuration changes.
5. Open a pull request with clear testing notes.

## Commit Policy

This repo uses strict atomic commits:

- one file per commit
- one change per commit
- conventional commit messages with a proper scope
- multiline structured commit bodies

Example:

```text
fix(service): handle idle reprompts

Why:
- remote SIP peers may drop quiet calls

What:
- add a configurable idle reprompt before the likely remote timeout

Refs:
- issue: ISSUE-123
```

Only include `Refs` footer lines for identifiers that actually exist.

## Pull Requests

Please include:

- what changed
- why it changed
- how it was tested
- rollout or risk notes if relevant

If your change affects users, APIs, deployment, or operations, update the relevant docs in the same branch.
