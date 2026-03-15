FROM debian:bookworm-slim

RUN useradd --system --uid 10001 --gid nogroup --create-home --home-dir /app agentvoice

WORKDIR /app

COPY target/release/agent_voice /usr/local/bin/agent_voice
COPY config /app/config
COPY deploy /app/deploy
COPY docs /app/docs
COPY README.md /app/README.md

RUN chmod +x /app/deploy/docker-entrypoint.sh

USER 10001:65534

ENTRYPOINT ["/app/deploy/docker-entrypoint.sh"]
