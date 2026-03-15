# Release Checklist

## Code

- `make test`
- `make lint`
- `make doc`

## Docs

- `make docs-install`
- `make docs-build`

## Runtime

- `make docker-build`
- `make docker-up`
- `curl -sS http://127.0.0.1:8089/healthz`
- `curl -sS http://127.0.0.1:8089/v1/status`

The Makefile-backed Compose targets use `.env` when it exists and otherwise fall back to `.env.example`, so release validation can build and render config without a secrets file.

## One-shot local release gate

- `make release-check`

## Live call checks

- inbound greeting works
- the assistant waits for the caller after greeting
- phone-book field questions are answered correctly
- email requires confirmation before it is saved
- accounting CSVs append correctly
- transcripts persist correctly
