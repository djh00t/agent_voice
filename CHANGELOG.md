# Changelog

## Unreleased

### Added

- Rustdoc coverage for the public crate, module, and service surface.
- A Docusaurus documentation site in `website/` backed by the repo `docs/` directory.
- Release-focused documentation for overview, configuration, control API, address-book rules, deployment, testing, and release checks.
- A top-level `Makefile` covering Rust validation, docs validation, and Docker workflows.
- Selectable local sherpa-onnx speech backends with Moonshine STT, Kokoro TTS, uv-managed Python dependencies, and mounted model assets for Docker deployments.
- Full-duplex realtime voice through OpenAI and xAI, including PCMU framing, barge-in truncation, and call lifecycle handling.

### Changed

- Phone-book guidance now limits editable caller fields to the active caller record.
- Phone-book updates now require email confirmation before persistence.
- Caller notes are treated as low-priority context instead of primary conversation steering.
- Post-TTS inbound suppression is configurable to reduce false self-triggered turns.
- `/v1/status` now reports the active STT/TTS backends and TTS model for runtime verification.
- Realtime calls document their split-pipeline and provider-specific limitations.

### Security

- The Docusaurus site dependencies are pinned to `3.9.2`, and the release workflow now includes a reproducible `npm ci` bootstrap through `make release-check`.
- API keys are redacted from debug output, and the xAI key is omitted from serialized configuration.
