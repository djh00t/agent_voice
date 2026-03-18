# agent_voice

`agent_voice` is a Rust SIP bridge for agent workflows.

It registers a SIP endpoint with `xphone`, exposes a localhost HTTP API for agents, turns inbound RTP into caller utterances, and can run either a split `STT -> LLM -> TTS` pipeline or a unified OpenAI voice-model reply path.

## Features

- SIP registration with UDP/TCP/TLS transport support through `xphone`
- Incoming and outgoing call handling
- Selectable OpenAI or local sherpa-onnx speech backends for STT and TTS
- Separately configurable standalone LLM and unified voice-model backends
- Inbound caller turn detection with OpenAI `POST /v1/audio/transcriptions` or local Moonshine
- Agent replies via OpenAI `POST /v1/responses` or OpenAI audio chat completions
- Outbound speech injection via OpenAI `POST /v1/audio/speech` or local Kokoro
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
- `LLM_PROVIDER`
- `VOICE_PROVIDER`
- `SPEECH_STT_PROVIDER`
- `SPEECH_TTS_PROVIDER`
- `OPENAI_TRANSCRIPTION_API_URL`
- `OPENAI_RESPONSES_API_URL`
- `OPENAI_RESPONSE_MODEL`
- `OPENAI_RESPONSE_INSTRUCTIONS`
- `OPENAI_VOICE_API_URL`
- `OPENAI_VOICE_MODEL`
- `OPENAI_VOICE_NAME`
- `OPENAI_VOICE_INSTRUCTIONS`
- `SHERPA_ONNX_PYTHON_BIN`
- `SHERPA_ONNX_BRIDGE_SCRIPT`
- `SHERPA_ONNX_STT_*`
- `SHERPA_ONNX_TTS_*`
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
On startup the app can refresh [accounting/models.json](accounting/models.json) from the official OpenAI pricing page and then use that mounted catalog for token and cost accounting.

## Local sherpa-onnx speech

Local speech uses an uv-managed Python environment plus the official `sherpa-onnx` package.

Bootstrap the local runtime:

```bash
make uv-sync
make sherpa-download-models
```

Then switch `.env` or YAML to:

```bash
SPEECH_STT_PROVIDER=sherpa_onnx
SPEECH_TTS_PROVIDER=sherpa_onnx
```

To keep local STT/TTS but disable the standalone LLM in favor of an OpenAI audio model:

```bash
LLM_PROVIDER=none
VOICE_PROVIDER=openai
OPENAI_VOICE_MODEL=gpt-audio-1.5
```

In that mode, inbound caller audio goes directly into the voice model by default (single API hop for reply generation + transcript), so STT is skipped unless you explicitly set:

```bash
OPENAI_VOICE_INPUT_TRANSCRIPTION_MODEL=whisper-1
```

If `OPENAI_VOICE_INPUT_TRANSCRIPTION_MODEL` is set, the pipeline reverts to a split STT → voice-model path for callers.

The default example paths expect:

- `./models/stt/moonshine`
- `./models/tts/kokoro`

The current local implementation supports:

- Moonshine for offline STT
- Kokoro for offline TTS
- persistent preloaded STT and TTS worker processes
- startup warmup so the first real greeting or caller turn does not pay the full model-load cost
- segmented local TTS playback so speech can start before the entire reply finishes synthesizing
- numeric `speaker_id` selection for built-in Kokoro voices

True custom voice creation is not part of the current runtime path yet.

## Local development

```bash
cargo fmt
make uv-sync
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

## Verification

You can run the local quality gates before deploying:

```bash
make test
make lint
```

Both gates are expected to pass for this update (and were validated in CI).

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
make uv-sync
make docker-pull
make docker-up
make docker-logs
```

The container runs from environment variables by default. If you prefer a mounted YAML file, add a bind mount and set `AGENT_VOICE_CONFIG=/app/config/agent_voice.yaml`.
The Makefile-backed Compose targets automatically use `.env` when present and fall back to `.env.example` for image pulls and config rendering.
Container images are published by GitHub Actions to `ghcr.io/djh00t/agent_voice`, and the tv04 deploy runner pulls that image before restarting the Compose service.
The bundled Compose file sets `AGENT_VOICE_CONFIG=/app/config/agent_voice.yaml` so the mounted YAML baseline is always readable inside the container, but environment variables still override that file on startup.
Use `AGENT_VOICE_IMAGE` if you need to pin Compose to a specific published tag such as `sha-<commit>` instead of the default `main` tag.
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
If you enable local sherpa-onnx speech, the Compose file also mounts `./models` at `/app/models`, and `/v1/status` reports `stt_backend` and `tts_backend` so you can confirm the active speech path.

## Deployment

The recommended deployment path is the bundled Compose stack at `/opt/agent_voice`, with GitHub Actions publishing the image to GHCR and the tv04 self-hosted runner updating that checkout, pulling the image, and restarting `agent-voice`.

Use the bundled systemd unit at [deploy/agent-voice.service](/opt/agent_voice/deploy/agent-voice.service) only for non-container deployments. An optional environment template is included at [deploy/agent-voice.env.example](/opt/agent_voice/deploy/agent-voice.env.example). The service expects:

- project path: `/opt/agent_voice`
- binary path: `/opt/agent_voice/target/release/agent_voice`
- config path: `/opt/agent_voice/config/agent_voice.yaml`

Detailed smoke-test steps are in [docs/testing.md](/opt/agent_voice/docs/testing.md).
