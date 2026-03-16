FROM rust:1.94-bookworm AS rust-builder

WORKDIR /build

COPY Cargo.toml Cargo.lock /build/
COPY src /build/src

RUN --mount=type=bind,source=.cargo/registry/cache,target=/usr/local/cargo/registry/cache,readonly \
    --mount=type=bind,source=.cargo/registry/index,target=/usr/local/cargo/registry/index,readonly \
    --mount=type=bind,source=.cargo/registry/src,target=/usr/local/cargo/registry/src,readonly \
    cargo build --release --offline

FROM python:3.10-slim-bookworm

COPY --from=ghcr.io/astral-sh/uv:0.10.2 /uv /uvx /usr/local/bin/
COPY --from=rust-builder /build/target/release/agent_voice /usr/local/bin/agent_voice

RUN useradd --system --uid 10001 --gid nogroup --create-home --home-dir /app agentvoice

WORKDIR /app

COPY pyproject.toml uv.lock /app/
COPY .venv /app/.venv
COPY python /app/python

ENV PATH="/app/.venv/bin:${PATH}"

RUN ln -sf /usr/local/bin/python3 /app/.venv/bin/python \
  && ln -sf python /app/.venv/bin/python3 \
  && ln -sf python /app/.venv/bin/python3.10

COPY config /app/config
COPY deploy /app/deploy
COPY docs /app/docs
COPY README.md /app/README.md

RUN chmod +x /app/deploy/docker-entrypoint.sh

USER 10001:65534

ENTRYPOINT ["/app/deploy/docker-entrypoint.sh"]
