# Control API

The local control API binds to `AGENT_API_LISTEN`, which defaults to `127.0.0.1:8089`.

## Endpoints

- `GET /healthz`
- `GET /v1/status`
- `GET /v1/calls`
- `GET /v1/calls/{call_id}`
- `GET /v1/calls/{call_id}/transcript`
- `POST /v1/dial`
- `POST /v1/calls/{call_id}/speak`
- `POST /v1/calls/{call_id}/hangup`

## Dial example

```bash
curl -sS -X POST http://127.0.0.1:8089/v1/dial \
  -H 'content-type: application/json' \
  -d '{"target":"sip:1002@example-pbx.local"}'
```

## Speak example

```bash
curl -sS -X POST http://127.0.0.1:8089/v1/calls/<call_id>/speak \
  -H 'content-type: application/json' \
  -d '{"text":"Testing the OpenAI TTS bridge."}'
```

## Status payload notes

`GET /v1/status` includes the active speech backends so you can confirm whether the runtime is using OpenAI or local sherpa-onnx for STT and TTS.

Relevant top-level fields:

- `phone_state`
- `stt_backend`
- `tts_backend`
- `tts_model`
- `calls`

## Transcript example

```bash
curl -sS http://127.0.0.1:8089/v1/calls/<call_id>/transcript | jq
```
