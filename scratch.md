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

| Metric | Avg | p50 | Min | Max |
| --- | ---: | ---: | ---: | ---: |
| Call duration | 27.961 s | 21.963 s | 3.442 s | 39.489 s |
| transcription | 154 ms | 143 ms | 106 ms | 259 ms |
| responses.reply | 1,921 ms | 1,444 ms | 1,222 ms | 4,517 ms |
| tts.reply | 7,194 ms | 4,884 ms | 1,942 ms | 24,300 ms |
| Full turn (trans+resp+tts) | 9,270 ms | 6,670 ms | 3,432 ms | 29,076 ms |
| Cost | $0.000113 | — | — | — |

## Notes

- Legacy split path is consistently cheaper but materially slower on these samples.
- New `gpt-audio-1.5` path is substantially faster end-to-end in the measured calls.
- Current production logs still show the `gpt-4o-transcribe` call in the same call path if `OPENAI_VOICE_INPUT_TRANSCRIPTION_MODEL` is set.
