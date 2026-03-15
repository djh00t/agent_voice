# agent_voice

`agent_voice` is a Rust SIP bridge for agent workflows.

It registers a SIP endpoint with `xphone`, exposes a localhost HTTP API for agents, turns inbound RTP into caller utterances, sends those turns to OpenAI speech-to-text and responses APIs, and sends outbound OpenAI TTS audio back into the SIP/RTP stream.

## Features

- SIP registration with UDP/TCP/TLS transport support through `xphone`
- Incoming and outgoing call handling
- Inbound caller turn detection with OpenAI `POST /v1/audio/transcriptions`
- Agent replies via OpenAI `POST /v1/responses`
- Outbound speech injection via OpenAI `POST /v1/audio/speech`
- Persistent JSON phone book keyed by caller ID
- Deny-by-default inbound caller-ID access control via the phone book
- Local control API for agents on `127.0.0.1`
- Unit-tested audio conversion and protocol event parsing

## Control API

- `GET /healthz`
- `GET /v1/status`
- `GET /v1/calls`
- `GET /v1/calls/{call_id}`
- `GET /v1/calls/{call_id}/transcript`
- `POST /v1/dial`
- `POST /v1/calls/{call_id}/speak`
- `POST /v1/calls/{call_id}/hangup`

### Dial a call

```bash
curl -sS -X POST http://127.0.0.1:8089/v1/dial \
  -H 'content-type: application/json' \
  -d '{"target":"sip:1002@example-pbx.local"}'
```

### Speak into an active call

```bash
curl -sS -X POST http://127.0.0.1:8089/v1/calls/<call_id>/speak \
  -H 'content-type: application/json' \
  -d '{"text":"Testing the OpenAI TTS bridge."}'
```

### Read transcript events

```bash
curl -sS http://127.0.0.1:8089/v1/calls/<call_id>/transcript | jq
```

## Configuration

The service now supports three config sources, in this order:

1. Built-in defaults
2. Optional YAML config file
3. Environment variable overrides

That means Docker and Compose can run it with environment variables only.

Copy [config/agent_voice.example.yaml](/Users/djh/agent_voice_work/config/agent_voice.example.yaml) to `config/agent_voice.yaml` if you want a file-based config, or copy [.env.example](/Users/djh/agent_voice_work/.env.example) to `.env` if you want Compose-driven config.

Important environment variables:

- SIP username, password, host, and transport details
- `OPENAI_API_KEY`
- `OPENAI_TRANSCRIPTION_API_URL`
- `OPENAI_RESPONSES_API_URL`
- `OPENAI_RESPONSE_MODEL`
- `OPENAI_RESPONSE_INSTRUCTIONS`
- `AGENT_API_LISTEN`
- `INCOMING_ANSWER_DELAY_MS`
- `INCOMING_GREETING_TEXT`
- `TRANSCRIPT_DIR`
- `PHONE_BOOK_PATH`
- `ACCOUNTING_MODEL_CATALOG_PATH`
- `ACCOUNTING_API_CALLS_CSV_PATH`
- `ACCOUNTING_CALL_TOTALS_CSV_PATH`
- `ACCOUNTING_PRICING_PAGE_URL`
- `ACCOUNTING_REFRESH_PRICING_ON_STARTUP`
- `ASSISTANT_NAME`
- `CALL_TURN_SILENCE_MS`
- `CALL_MIN_UTTERANCE_MS`
- `CALL_VAD_THRESHOLD`
- `CALL_CONTEXT_WINDOW_EVENTS`
- `AUTO_END_CALLS`
- `END_CALL_BUFFER_MS`

The binary auto-loads `./config/agent_voice.yaml` or `/opt/agent_voice/config/agent_voice.yaml` when present. If neither file exists, it runs from environment variables only.
The container entrypoint resolves hostname-style `SIP_HOST` values to IPv4 automatically before launch because `xphone` expects a socket-address target.
On startup the app can refresh [accounting/models.json](/Users/djh/agent_voice_work/accounting/models.json) from the official OpenAI pricing page and then use that mounted catalog for token and cost accounting.

## Local development

```bash
cargo fmt
cargo test
cargo doc --no-deps
cargo run -- --config ./config/agent_voice.yaml
```

The repository also includes a top-level `Makefile` so the common workflows can be driven consistently:

```bash
make docs-install
make check
make release-check
```

## Documentation

Rust API docs are written as Rust doc comments and can be built locally with:

```bash
cargo doc --no-deps
```

A Docusaurus site lives in `./website` and uses the repo `./docs` directory as its source:

```bash
make docs-install
make docs-build
```

The release documentation covers overview, configuration, control API, address-book behavior, deployment, testing, and the release checklist.
Release notes are tracked in `CHANGELOG.md`.

## Docker

Linux host networking is the simplest way to run SIP + RTP in Docker without fighting NAT or large UDP port mappings. The included Compose file uses `network_mode: host` for that reason.

```bash
cp .env.example .env
make docker-build
make docker-up
make docker-logs
```

The container runs from environment variables by default. If you prefer a mounted YAML file, add a bind mount and set `AGENT_VOICE_CONFIG=/app/config/agent_voice.yaml`.
The Makefile-backed Compose targets automatically use `.env` when present and fall back to `.env.example` for build and config rendering.
`make docker-build` first compiles `target/release/agent_voice` on the host and then packages that binary into the runtime image, so the Docker build does not need cargo registry access.
The bundled Compose file sets `AGENT_VOICE_CONFIG=/app/config/agent_voice.yaml` so the mounted YAML baseline is always readable inside the container, but environment variables still override that file on startup.
Compose defaults the container user to `0:0` so it can refresh the mounted pricing catalog and append accounting CSV rows on bind-mounted host directories. Override `AGENT_VOICE_UID` and `AGENT_VOICE_GID` if you want a different runtime user.
Inbound auto-answer delay is controlled with `INCOMING_ANSWER_DELAY_MS`. Set it to `2000` for a two-second delay before answering.
Inbound greeting text is controlled with `INCOMING_GREETING_TEXT`. Caller turn detection is tuned with `CALL_TURN_SILENCE_MS`, `CALL_MIN_UTTERANCE_MS`, and `CALL_VAD_THRESHOLD`.
The assistant identity is controlled by `ASSISTANT_NAME`. The JSON phone book path is controlled by `PHONE_BOOK_PATH`.
Inbound access control is enforced from `PHONE_BOOK_PATH`. Exact caller records are allowed unless `disabled: true`. Unknown caller IDs fall back to the `*` policy record, and callers without caller ID fall back to `__no_caller_id__`. Both policy entries are seeded as `disabled: true` by default, so inbound access is deny-by-default until you explicitly allow callers.
Conversation replay is bounded by `CALL_CONTEXT_WINDOW_EVENTS` so per-turn LLM latency stays flatter as calls get longer.
At `info` level, each detected caller turn logs `gap_since_previous_turn_ms`, `stt_ms`, `extraction_ms`, `llm_ms`, `tts_ms`, `total_turn_ms`, and running averages so you can see where time is going.
Each OpenAI call also logs token counts and `cost_usd`, while `GET /v1/status` and `GET /v1/calls/{call_id}` expose the running per-call totals. Detailed rows land in the mounted CSV files under `./accounting`.
`AUTO_END_CALLS` and `END_CALL_BUFFER_MS` are available for the call wrap-up path.
Transcripts are written under `TRANSCRIPT_DIR`. The Compose file mounts `./config`, `./accounting`, and `./data` so config files, the model catalog, CSV accounting, transcripts, and the phone book all persist on the host.

## Deployment

Use the bundled systemd unit at [deploy/agent-voice.service](/Users/djh/agent_voice_work/deploy/agent-voice.service). An optional environment template is included at [deploy/agent-voice.env.example](/Users/djh/agent_voice_work/deploy/agent-voice.env.example). The service expects:

- project path: `/opt/agent_voice`
- binary path: `/opt/agent_voice/target/release/agent_voice`
- config path: `/opt/agent_voice/config/agent_voice.yaml`

Detailed smoke-test steps are in [docs/testing.md](/Users/djh/agent_voice_work/docs/testing.md).
