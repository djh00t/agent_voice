FROM rust:1.94-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY config ./config
COPY deploy ./deploy
COPY docs ./docs
COPY README.md ./

RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --gid nogroup --create-home --home-dir /app agentvoice

WORKDIR /app

COPY --from=builder /app/target/release/agent_voice /usr/local/bin/agent_voice
COPY --from=builder /app/config /app/config
COPY --from=builder /app/deploy /app/deploy
COPY --from=builder /app/docs /app/docs
COPY --from=builder /app/README.md /app/README.md

RUN chmod +x /app/deploy/docker-entrypoint.sh

USER 10001:65534

ENTRYPOINT ["/app/deploy/docker-entrypoint.sh"]
