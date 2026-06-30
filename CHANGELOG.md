# Changelog

All notable changes to split-brain-harness are documented here.

---

## [1.0.0] — 2026-06-25

First stable release. Published to [crates.io](https://crates.io/crates/split-brain-harness).

### Architecture

Two-stage LLM security pipeline running as a drop-in OpenAI-compatible proxy:

```
input → [Stage 0: Normalizer] → [Stage 1: Propose] → [Stage 2: Verify] → HarnessResult
```

- **Stage 0** — deobfuscation normalizer; detects encoding-evasion attacks (Morse, Base64, homoglyphs, backslash-escape, Leet, URL-encoding) before the LLM ever sees the input. Extracted to the standalone [`deobfuscate`](https://crates.io/crates/deobfuscate) crate.
- **Stage 1: Propose** — soul-injected system prompt wraps every request; LLM returns structured `TelemetryResult` JSON with affective and intent telemetry.
- **Stage 2: Verify** — 6 deterministic consistency checks (always on) ± optional second LLM verifier pass. Safe structured fallback on non-JSON or refusals.

### Added

**Core pipeline**
- Two-stage propose/verify pipeline with soul-injected system prompts
- Safe structured fallback on model refusals and malformed JSON
- `stop_and_ask=true` enforcement when confidence < 0.4 or ≥ 3 flags fire

**Backends**
- Anthropic (claude-*), OpenAI-compatible (any), and Ollama (local, air-gapped) backends
- Fully offline operation against a local Ollama model

**Transformer / RAG**
- Transformer layer assembles soul + RAG corpus into the system prompt
- Adaptor layer: trigger-matched context packs injected per-request
- `sbh export-ollama` — embed RAG context docs into a Modelfile for Ollama

**Serve mode**
- Drop-in OpenAI-compatible proxy (`--serve`)
- TLS support (`axum-server` + rustls)
- `GET /metrics` — Prometheus counters and gauges
- Session escalation log — append-only JSONL witness feed

**Monitor TUI**
- `sbh-monitor` binary — real-time Ratatui TUI for live session monitoring (`monitor` feature)

**CLI**
- `demo` subcommand — 5 DHS-relevant threat scenarios (offline-capable)
- `demo --serve` — multi-turn slow-boil session escalation scenario
- `demo --export` — export demo session to file
- `--dump-prompt` / `--dump-raw` debug flags
- `debug-bundle` subcommand

**Benchmarks**
- Adversarial benchmark suite against three datasets:
  - **Deepset** (546 rows): precision 0.81 · recall 0.37 · F1 0.51
  - **CyberEC** (141 rows): precision **1.00** · recall 0.50 · F1 0.67 — zero false positives
  - **TrustAI** (1,398 unlabeled jailbreaks): **94.8% flagging rate**
- `sbh bench` — repeatable calibration benchmark with baseline diff

**Tests**
- 354 tests (unit + integration)
- `live-tests` feature flag for tests requiring a live Ollama backend

**Published**
- `split-brain-harness` v1.0.0 on crates.io
- MIT license
- DHS SBIR pitch deck (`SLIDES.md` — Marp-compatible)

---

## [0.1.0] — 2026-06-21

Initial prototype release.

### Added
- Soul-injected LLM telemetry harness with Anthropic, OpenAI-compat, and Ollama backends
- Two-stage propose-verify pipeline
- `config.toml` support
- CI workflow
- TUI monitor binary (`sbh-monitor`)
- Adaptor layer with compiled-in context packs
- Rate limiter, session eviction, input validation
