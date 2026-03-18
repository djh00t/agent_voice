# Deployment

## Docker Compose

Linux host networking is the simplest way to run SIP and RTP in Docker without large UDP port maps.

```bash
cp .env.example .env
make uv-sync
make docker-pull
make docker-up
make docker-logs
```

GitHub Actions builds and publishes the runtime image to `ghcr.io/djh00t/agent_voice`.
The tv04 self-hosted runner deploys by fast-forwarding `/opt/agent_voice`, pulling the published image, and restarting the `agent-voice` Compose service.
Compose defaults to `ghcr.io/djh00t/agent_voice:main`; set `AGENT_VOICE_IMAGE` in `.env` if you need to pin a specific published tag.

The Compose file mounts:

- `./config`
- `./accounting`
- `./data`
- `./models`

That keeps configuration, the pricing catalog, accounting CSVs, transcripts, the phone book, and any local sherpa-onnx model assets on the host.

## systemd

The repo also includes:

- `deploy/agent-voice.service`
- `deploy/agent-voice.env.example`

Those are useful for non-container deployments, though the current recommended runtime is the GHCR-backed Docker Compose path above.
