# Configuration

The service supports three config sources, in this order:

1. Built-in defaults
2. Optional YAML config file
3. Environment variable overrides

That makes the Docker and Compose workflow environment-first while still allowing mounted YAML for explicit deployments.

## Main groups

### SIP

- `SIP_USERNAME`
- `SIP_PASSWORD`
- `SIP_HOST`
- `SIP_PORT`
- `SIP_TRANSPORT`
- `SIP_RTP_PORT_MIN`
- `SIP_RTP_PORT_MAX`

### OpenAI

- `OPENAI_API_KEY`
- `OPENAI_TRANSCRIPTION_MODEL`
- `OPENAI_RESPONSE_MODEL`
- `OPENAI_TTS_MODEL`
- `OPENAI_TTS_VOICE`
- `OPENAI_TTS_INSTRUCTIONS`
- `OPENAI_RESPONSE_INSTRUCTIONS`

### Speech backends

- `SPEECH_STT_PROVIDER`
- `SPEECH_TTS_PROVIDER`
- `SHERPA_ONNX_PYTHON_BIN`
- `SHERPA_ONNX_BRIDGE_SCRIPT`
- `SHERPA_ONNX_PROVIDER`
- `SHERPA_ONNX_NUM_THREADS`
- `SHERPA_ONNX_WARMUP_ON_STARTUP`
- `SHERPA_ONNX_STARTUP_TIMEOUT_MS`
- `SHERPA_ONNX_REQUEST_TIMEOUT_MS`
- `SHERPA_ONNX_DEBUG`
- `SHERPA_ONNX_STT_MODEL_FAMILY`
- `SHERPA_ONNX_STT_MOONSHINE_PREPROCESSOR`
- `SHERPA_ONNX_STT_MOONSHINE_ENCODER`
- `SHERPA_ONNX_STT_MOONSHINE_UNCACHED_DECODER`
- `SHERPA_ONNX_STT_MOONSHINE_CACHED_DECODER`
- `SHERPA_ONNX_STT_MOONSHINE_DECODER`
- `SHERPA_ONNX_STT_MOONSHINE_TOKENS`
- `SHERPA_ONNX_TTS_MODEL_FAMILY`
- `SHERPA_ONNX_TTS_SPEED`
- `SHERPA_ONNX_TTS_SPEAKER_ID`
- `SHERPA_ONNX_TTS_KOKORO_MODEL`
- `SHERPA_ONNX_TTS_KOKORO_VOICES`
- `SHERPA_ONNX_TTS_KOKORO_TOKENS`
- `SHERPA_ONNX_TTS_KOKORO_DATA_DIR`
- `SHERPA_ONNX_TTS_KOKORO_LANG`

### Call behavior

- `INCOMING_ANSWER_DELAY_MS`
- `INCOMING_GREETING_TEXT`
- `ASSISTANT_NAME`
- `DEFAULT_TIMEZONE`
- `CALL_TURN_SILENCE_MS`
- `CALL_MIN_UTTERANCE_MS`
- `POST_TTS_INPUT_SUPPRESSION_MS`
- `CALL_VAD_THRESHOLD`
- `CALL_CONTEXT_WINDOW_EVENTS`
- `AUTO_END_CALLS`
- `END_CALL_BUFFER_MS`

### Persistence and accounting

- `TRANSCRIPT_DIR`
- `PHONE_BOOK_PATH`
- `ACCOUNTING_MODEL_CATALOG_PATH`
- `ACCOUNTING_API_CALLS_CSV_PATH`
- `ACCOUNTING_CALL_TOTALS_CSV_PATH`
- `ACCOUNTING_PRICING_PAGE_URL`
- `ACCOUNTING_REFRESH_PRICING_ON_STARTUP`

## Local sherpa-onnx speech

When `SPEECH_STT_PROVIDER=sherpa_onnx`, caller audio is transcribed locally with the repo-managed uv environment and the configured Moonshine model files.

When `SPEECH_TTS_PROVIDER=sherpa_onnx`, assistant speech is synthesized locally with the configured sherpa-onnx TTS model files. The current implementation supports Kokoro for local TTS and uses `SHERPA_ONNX_TTS_SPEAKER_ID` to select a built-in voice.

The runtime starts persistent preloaded sherpa-onnx workers for the enabled local backends. `SHERPA_ONNX_WARMUP_ON_STARTUP` controls whether those workers run a dummy inference during startup so the first real caller turn and greeting are hot. `SHERPA_ONNX_STARTUP_TIMEOUT_MS` and `SHERPA_ONNX_REQUEST_TIMEOUT_MS` control how long the Rust service will wait for worker readiness and individual synthesis/transcription requests.

The default sherpa worker thread count now follows host CPU parallelism and caps at `8` threads unless you override `SHERPA_ONNX_NUM_THREADS`.

The Compose stack mounts `./models` at `/app/models`, so the default local model paths are expected to exist under:

- `/app/models/stt/moonshine`
- `/app/models/tts/kokoro`

The repo-managed Python environment is expected at `./.venv` on the host and `/app/.venv` in Docker. Use `make uv-sync` to create it.

## Phone-book rules

Editable caller fields are:

- `first_name`
- `last_name`
- `email`
- `company`
- `timezone`
- `preferred_language`
- `notes`

Validation rules:

- `first_name` and `last_name` must clearly be the caller's own name.
- `email` must be confirmed before it is saved.
- `company` must clearly belong to the active caller.
- `timezone` must come from the caller's own explicit location.
- `preferred_language` must come from an explicit caller preference.
- `notes` are low-priority caller-specific memory, not the main conversational context.

## Inbound access control

Inbound caller-ID control is enforced through the phone book JSON.

- Exact caller records are allowed by default unless that record has `disabled: true`.
- The wildcard record `*` controls callers that do present caller ID but do not have an exact record.
- The special record `__no_caller_id__` controls callers that do not present caller ID.
- Both policy records are auto-seeded as `disabled: true`, so the default posture is deny-all for unknown caller IDs and deny-all for missing caller ID.

Example policy entries:

```json
{
  "callers": {
    "*": {
      "disabled": true,
      "system_entry": true
    },
    "__no_caller_id__": {
      "disabled": true,
      "system_entry": true
    },
    "61415850000": {
      "disabled": false,
      "first_name": "David"
    }
  }
}
```
