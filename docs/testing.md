# Testing

## Local checks

Run these before copying to `tv04`:

```bash
cargo fmt
make uv-sync
cargo test
cargo clippy --all-targets --all-features -- -D warnings
docker compose config
```

## Local speech model bootstrap

If you want to exercise local sherpa-onnx speech instead of OpenAI STT/TTS:

```bash
make sherpa-download-models
cp .env.example .env
```

Then set these in `.env`:

```bash
SPEECH_STT_PROVIDER=sherpa_onnx
SPEECH_TTS_PROVIDER=sherpa_onnx
SHERPA_ONNX_WARMUP_ON_STARTUP=true
```

The default example paths assume:

- Moonshine files under `./models/stt/moonshine`
- Kokoro files under `./models/tts/kokoro`

## Docker smoke test

1. Copy `.env.example` to `.env`.
2. Fill in the SIP and OpenAI values.
   Leave `AGENT_VOICE_CONFIG=/app/config/agent_voice.yaml` so the mounted YAML baseline is used inside the container.
   Set `INCOMING_ANSWER_DELAY_MS=2000` if you want a two-second pause before answering inbound calls.
   Set `INCOMING_GREETING_TEXT=Welcome` to speak a greeting after answer.
   Set `OPENAI_RESPONSE_INSTRUCTIONS` if you want to change the agent persona.
   Set `ASSISTANT_NAME=Steve` or whatever caller-facing name you want the agent to use.
   Set `CALL_CONTEXT_WINDOW_EVENTS=8` to keep only a bounded recent window in LLM context.
   Leave `ACCOUNTING_REFRESH_PRICING_ON_STARTUP=true` if you want the container to refresh `./accounting/models.json` from the official OpenAI pricing page at boot.
   Set `SPEECH_STT_PROVIDER=sherpa_onnx` and `SPEECH_TTS_PROVIDER=sherpa_onnx` if you want local Moonshine STT plus local Kokoro TTS instead of OpenAI speech.
   Leave `SHERPA_ONNX_WARMUP_ON_STARTUP=true` if you want the container to preload and warm the local speech workers at boot.
3. Build and start:

```bash
docker compose build
docker compose up -d
docker compose logs -f
```

4. Verify the control API:

```bash
curl -sS http://127.0.0.1:8089/healthz
curl -sS http://127.0.0.1:8089/v1/status | jq
```

   Confirm `/v1/status` reports the expected `stt_backend` and `tts_backend`.
   Watch the startup logs for `persistent sherpa-onnx worker ready` so you know the local speech models are preloaded before the first call.

5. Place or receive a SIP call.
6. Confirm the call answers after two seconds and plays `Welcome`.
7. Speak into the call and wait for the agent to answer you.
   Watch `docker compose logs -f` for per-turn timing lines that include `gap_since_previous_turn_ms`, `stt_ms`, `extraction_ms`, `llm_ms`, `tts_ms`, and `total_turn_ms`.
   Watch for `recorded TTS accounting entry` and `recorded API accounting entry` lines to confirm cost and token details are being logged.
8. Hang up and check `./data/transcripts` for the saved caller and assistant transcript.
9. Check `./data/phone_book.json` to confirm caller details are being remembered by caller ID. Editable fields are `first_name`, `last_name`, `email`, `company`, `timezone`, `preferred_language`, and `notes`.
   Confirm that the seeded `*` and `__no_caller_id__` policy entries exist and default to `disabled: true`.
   Confirm that a caller with an exact record and `disabled: false` is accepted.
   Confirm that an exact record with `disabled: true` is rejected before answer.
   Confirm that an unknown caller is rejected while `*` remains disabled.
10. Email should not be written immediately. The agent should read it back and get a confirmation first; only then should it appear in the phone book.
11. Check `./accounting/api_calls.csv` and `./accounting/call_totals.csv` for per-request and per-call token/cost accounting.
    When local sherpa-onnx STT/TTS is active, the speech rows should show `local://sherpa-onnx/...` endpoints and zero API cost for those speech turns.

## Remote build

```bash
cd /opt/agent_voice
cargo build --release
cargo test
```

## Remote smoke test

1. Put a real SIP config at `/opt/agent_voice/config/agent_voice.yaml`.
2. Export `OPENAI_API_KEY` or set it in the config.
3. Start the service:

```bash
cd /opt/agent_voice
RUST_LOG=info,agent_voice=debug cargo run --release -- --config /opt/agent_voice/config/agent_voice.yaml
```

4. In another shell, verify the API:

```bash
curl -sS http://127.0.0.1:8089/healthz
curl -sS http://127.0.0.1:8089/v1/status | jq
```

5. Place or receive a SIP call.
6. Confirm the active call appears under `/v1/calls`.
7. Speak into the call and confirm transcript events appear under `/v1/calls/{call_id}/transcript`.
8. Confirm the agent generates a spoken reply automatically.
9. Optionally push manual TTS back into the call:

```bash
curl -sS -X POST http://127.0.0.1:8089/v1/calls/<call_id>/speak \
  -H 'content-type: application/json' \
  -d '{"text":"This is a remote smoke test of the SIP bridge."}'
```

10. Hang up cleanly:

```bash
curl -sS -X POST http://127.0.0.1:8089/v1/calls/<call_id>/hangup
```
