# Changelog

All notable changes to split-brain-harness are documented here.

---

## [1.3.0] — 2026-07-18

The first release since 1.2.0. It bundles the **v1.5 active-reconciliation loop**,
the **v2 "clean-core" workspace re-architecture** (behaviour-preserving —
benchmark parity held), and the full **advanced tier** (B–G). Every new command
is additive; existing commands are unchanged. Confidence and detection behaviour
are unaffected by default (new stages are gated/off unless configured).

### Added

**Active reconciliation (v1.5)**
- `analyze` is now a bounded propose → verify → adjudicate loop. A pure-rules
  arbitrator decides accept / re-refine / escalate; the best iteration is kept
  (guarded against regression). Config: `arbitrator` (`off` | `rules`, default
  `rules`), `refine_max_iters`, `refine_confidence_target`, `stop_and_ask_threshold`
  (envs `SBH_ARBITRATOR`, `SBH_REFINE_ITERS`, `SBH_REFINE_TARGET`, `SBH_STOP_THRESHOLD`).
  `arbitrator=off` or `refine_max_iters<=1` reproduces the byte-identical one-shot.
- Two new consistency checks: `scope-creep / hidden-payload` and
  `value-alignment delta` (8 deterministic checks total).
- Structured `DisagreementScore` + `CheckOutcome` model (dimension/weight per
  check) replaces substring-decoded scoring.

**Confidence calibration**
- `sbh calibrate [--store <path>]` fits offline Platt scaling from labelled
  feedback; `sbh feedback --fingerprint <fp> (--correct | --misread)` records
  outcome labels. Append-only JSONL, privacy-by-fingerprint (no raw input stored).
  Uncalibrated behaviour is identity — enabling a store is a no-op until fitted.

**Observability (tier B)**
- `sbh visualize [<trace.json>] [--output <out.html>]` renders a HarnessResult to
  a single self-contained offline HTML report (risk, telemetry, verification,
  intent, tool-risk, per-stage timing, refinement, trace).
- New `serve` `/metrics` counters: `sbh_flagged_total`, `sbh_stop_and_ask_total`,
  `sbh_refinement_iterations_total`, and (below) per-stage counters.

**Tool-aware telemetry (tier C)**
- Deterministic tool-risk classifier (code-exec / web / file-write / network /
  shell) cross-checked against the actual capability request, never LLM
  self-report. Surfaced additively as `HarnessResult.tool_risk`.

**HITL weight tuning (tier D)**
- `sbh tune-weights [--store <path>]` correlates each check's firing with feedback
  labels and prints per-check correct-when-fired rates with an advisory direction.
  Advisory only — never auto-applied.

**Formal verification (tier F)**
- Deterministic, LLM-free predicate engine over TOML rule domains, running after
  the reconcile loop. High-severity violations force `stop_and_ask`; a bad rules
  file fails closed. `sbh formal-check <rules> [input]` lints and dry-runs offline.
  `sbh_formal_violations_total` metric; starter `rules/credential-egress.toml`.

**Adversarial debate (tier E.1)**
- Gated devil's-advocate LLM pass (`advocate_mode` `off` | `high_stakes` | `always`,
  env `SBH_ADVOCATE`). Raise-only guardrail: dissent can add caution / force
  `stop_and_ask`, never clear a flag. Transient failure is advisory (not fail-closed).
  `sbh_advocate_dissent_total` metric.

**Per-request LLM-call budget (tier E.2)**
- `max_llm_calls_per_request` (env `SBH_MAX_LLM_CALLS`) meters and caps generate()
  calls across a run; the optional advocate skips cleanly when the budget is
  exhausted. `HarnessResult.llm_calls` + `sbh_llm_calls_total` metric. Unlimited by
  default.

**Offline meta-cognition (tier G)**
- `sbh introspect [--store <p>] [--session-log <p>] [--min-cluster <n>] [--json]`
  clusters *misread* runs by feature signature into archetypes (under-detection /
  checks-fired-but-wrong / fingerprint-misfire) and prints **advisory** weight and
  prompt suggestions with concrete weight diffs. Offline, deterministic, and
  writes nothing — runtime self-modification is a non-goal. Privacy-preserving
  (clusters off structured features, since stores hold only fingerprints).

### Changed

**v2 "clean-core" workspace re-architecture (behaviour-preserving)**
- The crate is now a Cargo workspace: leaf crates `sbh-normalize`, `sbh-core`,
  `sbh-store`, `sbh-llm`, `sbh-safety`, `sbh-forge` are carved out and re-exported
  from the root so existing paths resolve unchanged.
- `analyze()` decomposed into a timed stage pipeline (normalize → reconcile →
  obfuscation → calibration), each stage wall-clock-timed into the trace.
- Tolerant `Risk` enum (bad model values → `Risk::Unknown`, still flagged) and a
  strict `ModelContract` parse boundary.
- **Benchmark parity gate passed** (≈95% row-by-row agreement vs 1.2.0, precision
  parity exact) — the refactor is behaviour-preserving.

### Fixed
- UTF-8 panic in the normalizer's Stage 0 trace formatting on multibyte input
  (byte-slice → char-safe truncation).
- `parse_verify_mode` never mapped `"reconcile"` (silently fell back to deterministic).

### Notes
- crates.io publishing is paused while the crate is a workspace (unversioned path
  deps on unpublished members); this release is GitHub-only. crates.io remains at
  1.2.0.

---

## [1.2.0] — 2026-07-02

### Security

**`security` module — path validation and secret redaction**
- New `src/security.rs` with two hand-rolled utilities (no regex dependency, keeping third-party surface minimal)
- `validate_soul_path()`: `SBH_SOUL_PATH` / `soul_path` is now canonicalized before reading — must resolve to a regular `.md` file inside cwd, `$HOME`, or `/usr/share/sbh`. Blocks symlink traversal to arbitrary files (e.g. a symlink to `/etc/passwd`)
- `redact()`: scrubs `key=value` credentials (password, token, api_key, …), bearer tokens, well-known token prefixes (`sk-`, `ghp_`, `AKIA`, JWT, Slack), email addresses, and SSNs
- Trace evidence in the harness (propose-stage input excerpt, normalizer detection evidence) is now redacted before entering the trace, so secrets in user input no longer persist in JSON trace output

**Bounded detection vector in the normalizer**
- `NormalizationResult::detections` is now capped at `MAX_DETECTIONS` (100). Pathological inputs with thousands of interleaved obfuscation spans previously grew the vector without bound. Passes still normalize the full text; only the per-span evidence list is bounded. `summary()` notes `(capped)` when the cap is hit

### Added

**Refusal-graded fallback (fewer false positives)**
- Non-JSON model responses are now classified before falling back: refusal phrases ("I can't…", "I'm sorry…", "…ethical reasons") scanned in the first 200 chars
- Graded fallback risk: benign refusal with no injection packs active → `low` (previously `medium`); non-refusal garbage → `medium`; anything with injection packs active → `high` (unchanged)
- Fallback telemetry carries `model_refusal` vs `parse_failure` in `structural_tone`, and the trace entry names the refusal kind

**Configurable sampling temperature + verifier randomness discount**
- `temperature` config field (env `SBH_TEMPERATURE`, default 0.1), validated to 0.0–2.0, forwarded to all backends — the Anthropic backend previously used the API default (1.0); it now defaults to 0.1 like the others
- `verifier::randomness_discount()`: above temperature 0.5 the verifier discounts confidence linearly (up to 0.2 at 1.5+) so borderline `stop_and_ask` gates fail closed consistently instead of flipping with the sampling seed; a `verify-randomness` trace entry records the discount
- `verifier::verify()` now takes the proposer temperature as a parameter

**Reconcile chaos tests**
- Adjudicator failure modes now covered: parse failure, empty response, timeout/engine error — all degrade gracefully to a full report with a failed `verify-reconcile` trace entry; plus a guard test that the adjudicator does not fire without a fingerprint or high flag density

### Performance
- Morse reverse-lookup table is built once (`OnceLock`) instead of per decoded span

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
