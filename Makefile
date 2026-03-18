.DEFAULT_GOAL := help

DOCS_DIR := website
COMPOSE_ENV_FILE := $(if $(wildcard .env),.env,.env.example)
UV_CACHE_DIR ?= /tmp/uv-cache
SHERPA_MODELS_DIR ?= models
SHERPA_MOONSHINE_URL ?= https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-int8.tar.bz2
SHERPA_MOONSHINE_DIR ?= sherpa-onnx-moonshine-tiny-en-int8
SHERPA_KOKORO_URL ?= https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2
SHERPA_KOKORO_DIR ?= kokoro-multi-lang-v1_1

.PHONY: help fmt test lint doc uv-lock uv-sync local-speech-check sherpa-download-moonshine sherpa-download-kokoro sherpa-download-models build-bin release-bin docs-install docs-build docs-serve docs-audit check docker-pull docker-build docker-up docker-logs docker-down compose-config release-check clean

help:
	@printf '%s\n' \
		'make fmt            - format Rust code' \
		'make test           - run Rust tests' \
		'make lint           - run clippy with warnings denied' \
		'make doc            - build Rust API docs' \
		'make uv-lock        - refresh the uv.lock for local Python speech deps' \
		'make uv-sync        - create the repo-managed uv virtualenv for local speech' \
		'make local-speech-check - verify the local sherpa-onnx Python bridge can start' \
		'make sherpa-download-moonshine - download the example Moonshine STT model set' \
		'make sherpa-download-kokoro - download the example Kokoro TTS model set' \
		'make sherpa-download-models - download both example local speech model sets' \
		'make build-bin      - build the host debug Rust binary' \
		'make release-bin    - build the release Rust binary' \
		'make docs-install   - install Docusaurus dependencies' \
		'make docs-build     - build the Docusaurus site' \
		'make docs-serve     - serve the built Docusaurus site' \
		'make docs-audit     - audit the docs npm dependencies' \
		'make check          - run Rust and docs validation' \
		'make docker-pull    - pull the runtime container image using .env or .env.example' \
		'make docker-build   - compatibility alias for docker-pull' \
		'make docker-up      - start the local Compose stack using .env or .env.example' \
		'make docker-logs    - tail Compose logs' \
		'make docker-down    - stop the local Compose stack' \
		'make compose-config - render the Compose config using .env or .env.example' \
		'make release-check  - run the full local release checklist' \
		'make clean          - remove generated Rust and docs build output'

fmt:
	cargo fmt

test:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

doc:
	cargo doc --no-deps

uv-lock:
	mkdir -p $(UV_CACHE_DIR)
	UV_CACHE_DIR=$(UV_CACHE_DIR) uv lock

uv-sync:
	mkdir -p $(UV_CACHE_DIR)
	UV_CACHE_DIR=$(UV_CACHE_DIR) uv sync --frozen

local-speech-check: uv-sync
	./.venv/bin/python ./python/sherpa_onnx_bridge.py --help

sherpa-download-moonshine:
	mkdir -p $(SHERPA_MODELS_DIR)/stt
	curl -L $(SHERPA_MOONSHINE_URL) -o $(SHERPA_MODELS_DIR)/stt/$(SHERPA_MOONSHINE_DIR).tar.bz2
	cd $(SHERPA_MODELS_DIR)/stt && tar -xf $(SHERPA_MOONSHINE_DIR).tar.bz2
	rm -f $(SHERPA_MODELS_DIR)/stt/$(SHERPA_MOONSHINE_DIR).tar.bz2
	ln -sfn $(SHERPA_MOONSHINE_DIR) $(SHERPA_MODELS_DIR)/stt/moonshine

sherpa-download-kokoro:
	mkdir -p $(SHERPA_MODELS_DIR)/tts
	curl -L $(SHERPA_KOKORO_URL) -o $(SHERPA_MODELS_DIR)/tts/$(SHERPA_KOKORO_DIR).tar.bz2
	cd $(SHERPA_MODELS_DIR)/tts && tar -xf $(SHERPA_KOKORO_DIR).tar.bz2
	rm -f $(SHERPA_MODELS_DIR)/tts/$(SHERPA_KOKORO_DIR).tar.bz2
	ln -sfn $(SHERPA_KOKORO_DIR) $(SHERPA_MODELS_DIR)/tts/kokoro

sherpa-download-models: sherpa-download-moonshine sherpa-download-kokoro

build-bin:
	cargo build

release-bin:
	cargo build --release

docs-install:
	cd $(DOCS_DIR) && npm ci

docs-build:
	cd $(DOCS_DIR) && npm run build

docs-serve:
	cd $(DOCS_DIR) && npm run serve

docs-audit:
	cd $(DOCS_DIR) && npm audit

check: test lint doc docs-build

docker-pull:
	docker compose --env-file $(COMPOSE_ENV_FILE) pull

docker-build: docker-pull

docker-up:
	docker compose --env-file $(COMPOSE_ENV_FILE) up -d

docker-logs:
	docker compose logs -f

docker-down:
	docker compose down

compose-config:
	docker compose --env-file $(COMPOSE_ENV_FILE) config

release-check: local-speech-check docs-install test lint doc docs-build docker-pull compose-config

clean:
	rm -rf target $(DOCS_DIR)/build $(DOCS_DIR)/.docusaurus
