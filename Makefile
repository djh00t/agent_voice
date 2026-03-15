.DEFAULT_GOAL := help

DOCS_DIR := website
COMPOSE_ENV_FILE := $(if $(wildcard .env),.env,.env.example)

.PHONY: help fmt test lint doc release-bin docs-install docs-build docs-serve docs-audit check docker-build docker-up docker-logs docker-down compose-config release-check clean

help:
	@printf '%s\n' \
		'make fmt            - format Rust code' \
		'make test           - run Rust tests' \
		'make lint           - run clippy with warnings denied' \
		'make doc            - build Rust API docs' \
		'make release-bin    - build the release Rust binary for Docker packaging' \
		'make docs-install   - install Docusaurus dependencies' \
		'make docs-build     - build the Docusaurus site' \
		'make docs-serve     - serve the built Docusaurus site' \
		'make docs-audit     - audit the docs npm dependencies' \
		'make check          - run Rust and docs validation' \
		'make docker-build   - build the runtime container using .env or .env.example' \
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

docker-build: release-bin
	docker compose --env-file $(COMPOSE_ENV_FILE) build

docker-up:
	docker compose --env-file $(COMPOSE_ENV_FILE) up -d

docker-logs:
	docker compose logs -f

docker-down:
	docker compose down

compose-config:
	docker compose --env-file $(COMPOSE_ENV_FILE) config

release-check: docs-install test lint doc docs-build release-bin docker-build compose-config

clean:
	rm -rf target $(DOCS_DIR)/build $(DOCS_DIR)/.docusaurus
