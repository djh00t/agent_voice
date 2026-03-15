# Deployment

## Docker Compose

Linux host networking is the simplest way to run SIP and RTP in Docker without large UDP port maps.

```bash
cp .env.example .env
docker compose build
docker compose up -d
docker compose logs -f
```

The Compose file mounts:

- `./config`
- `./accounting`
- `./data`

That keeps configuration, the pricing catalog, accounting CSVs, transcripts, and the phone book on the host.

## systemd

The repo also includes:

- `deploy/agent-voice.service`
- `deploy/agent-voice.env.example`

Those are useful for non-container deployments, though the current recommended runtime is Docker Compose.
