# Changelog

All notable changes to split-brain-harness are documented here.

---

## [1.1.0] — 2026-06-30

### Added

**DisagreementScore — structured inter-agent disagreement analysis (DiscoUQ-inspired)**
- `DisagreementScore` struct in `types.rs`: replaces the flat per-flag confidence penalty in `derive_confidence()` with a structure-aware score
- Fields: `flag_count`, `flag_density` (fraction of 6 total checks that fired), `dimension_spread` (number of distinct analytical domains with at least one flag), `injection_fingerprint`, `adjusted_confidence`, `reconcile_verdict`
- `compute_disagreement_score()` in `verifier.rs` — public function, visible for calibration tooling
- Confidence formula: `base(coherence) − density_penalty(flag_density × 0.40) − spread_discount(0.05 when flags cluster in one domain) − fingerprint_penalty(0.25 if fingerprint)`, blended with LLM confidence if available
- `disagreement: DisagreementScore` field always populated in `VerificationReport`

**Injection fingerprint detection**
- Fires when `structural_tone` flag AND `urgency_vector` flag both triggered while the proposer asserted `manipulation_risk = "low"` — the canonical manipulation-evasion pattern (adversarial pressure + manufactured urgency camouflaged as low risk)
- Hard 0.25 confidence penalty when fingerprint fires

**VerifyMode::Reconcile — third adjudicator LLM pass (ReConcile-inspired)**
- New `VerifyMode::Reconcile` variant alongside `Deterministic`, `Llm`, and `None`
- When mode is `Reconcile`: runs deterministic checks + LLM verifier; if `injection_fingerprint` fires or `flag_density ≥ 0.5`, fires a third adjudicator LLM call
- Adjudicator receives original input + telemetry JSON + flag list; returns `{verdict, reasoning, confidence}` JSON
- Verdict summary stored in `disagreement.reconcile_verdict`; adjudicator confidence blended into `adjusted_confidence`
- Trace entry appended for full auditability

**Tests**
- 7 new `DisagreementScore` unit tests covering: clean input (no flags), injection fingerprint fires on tone+urgency+low-risk, fingerprint blocked when only one signal present, fingerprint blocked for high-risk assertions, dimension spread clustered vs spread, LLM confidence blending, flag density proportionality
- `verify_mode_reconcile_display` test for `Display` impl
- Total: 362 tests (290 unit + 72 integration/CLI/eval)

### Changed
- `derive_confidence()` removed — superseded by `compute_disagreement_score()`
- All existing verifier tests updated to use `confidence_from()` helper (delegates to `compute_disagreement_score`)

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
