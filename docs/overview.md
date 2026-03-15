# Overview

`agent_voice` is a Rust SIP bridge for agent workflows.

It registers a SIP endpoint with `xphone`, exposes a localhost HTTP API for agents, turns inbound RTP into caller utterances, sends those turns to OpenAI speech-to-text and responses APIs, and sends outbound OpenAI TTS audio back into the SIP/RTP stream.

## Core capabilities

- SIP registration plus inbound and outbound call handling
- RTP audio ingestion and telephony turn detection
- OpenAI STT, responses, and TTS integration
- Persistent caller phone book keyed by caller ID
- Per-call accounting, token tracking, and cost logging
- Local control API for agents on `127.0.0.1`

## Runtime model

The service keeps one runtime call record per active SIP call. Each record owns:

- transcript history
- phone-book context for the active caller ID
- accumulated accounting
- pending phone-book confirmations such as email confirmation

The active caller record is always keyed by the current SIP caller ID. The assistant should never write another person's details into that record.
