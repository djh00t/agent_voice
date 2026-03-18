# Voice Model Latency & Cost Stats (Current Dataset)

## Source window

- Data files: `accounting/api_calls.csv`, `accounting/call_totals.csv`
- Generated from container logs after `gpt-audio-1.5` activation on tv04.
- Reference date: 2026-03-18.

## Side-by-side runtime comparison

| Mode | Turn metric | Samples | Avg latency | p50 | p95 | Avg cost / call |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Legacy split | `transcription + responses.reply + tts.reply` | 17 turns | 12,066 ms | 8,225 ms | 29,076 ms | $0.000982 |
| New direct voice model | `voice.reply` (`gpt-audio-1.5`) | 4 turns | 3,617 ms | 3,613 ms | 3,673 ms | $0.022258 |
| New hybrid (with `gpt-4o-transcribe` still active) | `transcription + voice.reply` | 4 turns | 4,628 ms (paired estimate) | 4,628 ms | 4,628 ms | $0.022258 |

## Moonlight local stack snapshot (legacy local speech)

`sherpa-onnx-moonshine-v1` calls (STT) + `sherpa-onnx-kokoro` (TTS), OpenAI LLM still `gpt-4o-mini`: 6 calls.

### Snapshot

- Call IDs: 
  - `106d28773d4d184a50c458084986e551@10.0.6.10:5050`
  - `60f3d23e29f9b5c95d6bd6133f9ed3ae@10.0.6.10:5050`
  - `2ea64b3c2aa1e30b0d7af3630f5f8546@10.0.6.10:5050`
  - `642bb09c40bd31641decb7d53524ee20@10.0.6.10:5050`
  - `3ddf303009a260982e526bad04cbbaec@10.0.6.10:5050`
  - `5b6d979a5f57dc0d2fe2d57e70261980@10.0.6.10:5050`

| Metric | Avg | p50 | Min | Max |
| --- | ---: | ---: | ---: | ---: |
| Call duration | 27.961 s | 21.963 s | 3.442 s | 39.489 s |
| transcription | 154 ms | 143 ms | 106 ms | 259 ms |
| responses.reply | 1,921 ms | 1,444 ms | 1,222 ms | 4,517 ms |
| tts.reply | 7,194 ms | 4,884 ms | 1,942 ms | 24,300 ms |
| Full turn (trans+resp+tts) | 9,270 ms | 6,670 ms | 3,432 ms | 29,076 ms |
| Cost | $0.000113 | — | — | — |

### Component timings (ms, from accounting api call rows)

- STT (`transcription`): `154 ms` average, `143 ms` median
- LLM (`responses.reply`): `1,921 ms` average, `1,444 ms` median
- TTS (`tts.reply`): `7,194 ms` average, `4,884 ms` median
- Turn aggregate (`transcription + responses.reply + tts.reply`): `9,270 ms` average

## Notes

- Legacy split path is consistently cheaper but materially slower on these samples.
- New `gpt-audio-1.5` path is substantially faster end-to-end in the measured calls.
- Current production logs still show the `gpt-4o-transcribe` call in the same call path if `OPENAI_VOICE_INPUT_TRANSCRIPTION_MODEL` is set.
