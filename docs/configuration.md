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
