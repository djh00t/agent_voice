# Overview

`agent_voice` is a Rust SIP bridge for agent workflows.

It registers a SIP endpoint with `xphone`, exposes a localhost HTTP API for agents, turns inbound RTP into caller utterances, sends those turns to the configured speech backend, calls OpenAI Responses for reasoning, and sends outbound speech back into the SIP/RTP stream.

## Core capabilities

- SIP registration plus inbound and outbound call handling
- RTP audio ingestion and telephony turn detection
- Selectable OpenAI or local sherpa-onnx speech backends
- OpenAI Responses integration for reasoning
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
