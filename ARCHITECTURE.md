# Split-Brain Harness — Architecture & v2 Migration Spec

**Status:** frozen reference for the v2 "clean core" re-architecture.
**Baseline commit:** `53f9d1f` (branch `feat/v1.5-active-reconciliation`, v1.5 "active
reconciliation", 412 tests green). This document is the outline; no v1.5 behavior changes until
the v2 parity gate (P5) is met.

---

## 1. What SBH is

A soul-injected security layer that wraps any LLM and reads every input *before* a response is
generated, detecting prompt injection, insider-threat patterns, authority impersonation, and
multi-turn escalation. Two "hemispheres" — a **Proposer** (reads intent/affect/urgency) and a
**Verifier** (deterministic consistency audit) — with **disagreement between them as the alarm**.
Ships as a single static Rust binary; runs air-gapped against a local model.

Design principles (from the project briefing) that constrain every decision here:
minimal third-party surface, compartmentalization over consolidation, soul files are identity,
witness layer woven in, simplicity as a correctness signal.

---

## 2. Current system (as-is)

Single crate, **30 modules** under `src/`, wired via `src/lib.rs`. Async (`tokio`), minimal deps
(`serde`, `reqwest`, `anyhow`, `toml`, `base64`; `axum` behind `serve`, `ratatui` behind `monitor`).

### 2.1 Pipeline (`src/harness.rs :: analyze()`)

```
input
  → Stage 0  Normalizer      (src/normalizer.rs)   7-pass deobfuscation
  → Adaptor                  (src/adaptor.rs)      trigger-matched context packs
  → Transformer              (src/transformer.rs)  soul + RAG → system prompt
  → Stage 1  PROPOSE (LLM)   → Telemetry JSON  (+ rationale, A1)
  → [refinement loop, A4]    ← arbitrator (src/arbitrator.rs) decides accept/re-refine/escalate
  → Stage 2  VERIFY          (src/verifier.rs)     6+2 deterministic checks ± LLM verifier ± reconcile
  → obfuscation override
  → calibration log/apply    (src/calibration.rs, A5)
  → HarnessResult { telemetry, verification, trace, capability_request?, obfuscation?, refinement? }
```

### 2.2 Modules by concern

| Concern | Modules |
|---|---|
| Core pipeline | `harness`, `transformer`, `adaptor`, `soul`, `types`, `extractor`, `capability` |
| Verification | `verifier`, `arbitrator`, `calibration` |
| Stage 0 | `normalizer` (+ standalone `deobfuscate`/`unicode-interference` crates) |
| LLM I/O | `backends/{mod,ollama,openai,anthropic,embedded}` |
| Retrieval | `rag`, `context_packs` (keyword/trigger only — **no embeddings**) |
| Stores (JSONL) | `audit`, `session_log`, `calibration`; `tool_memory` (whole-file JSON) |
| Forge | `generative_forge`, `regenerative_forge`, `tool_forge`, `wasm_forge`, `code_gen`, `static_analysis`, `reputation`, `capability` |
| Serve/observe | `serve` (proxy + hand-rolled Prometheus), `bin/monitor` (TUI) |
| Config/safety | `config`, `input_validation`, `security`, `policy` |

### 2.3 Data model (`src/types.rs`, `src/capability.rs`)

- `TelemetryResult { affective_telemetry, intent_matrix, cognitive_state }` — the three
  sub-structs carry `#[serde(deny_unknown_fields)]`.
- `VerificationReport { passed, consistency_flags: Vec<String>, …, confidence, disagreement, stop_and_ask }`.
- `DisagreementScore { flag_count, flag_density, dimension_spread, injection_fingerprint, adjusted_confidence, reconcile_verdict? }`.
- `HarnessResult { telemetry, verification, trace: Vec<TraceEntry>, capability_request?, obfuscation?, refinement? }`.
- `Config { … 20+ fields … }` + parallel `FileConfig` + `build_config()` + `validate_config()`.

### 2.4 Benchmark numbers — the acceptance criteria for v2

Local `ollama` `llama3.2:3b`, air-gapped. Fixtures + result JSONL live in `fixtures/`; drivers in
`scripts/` (`run_all_benches.sh`, `run_bench_labeled.py`, `run_bench_adversarial.py`) and
`examples/norm_bench.rs`.

| Dataset | Rows | Result |
|---|---|---|
| Deepset | 546 | Precision **0.81** · Recall 0.37 · F1 0.51 |
| CyberEC | 141 | Precision **1.00** · Recall 0.50 · F1 0.67 (+ normalizer catches 50% of encoding-evasion FNs) |
| TrustAI | 1,398 | **94.8%** flagging rate (unlabeled jailbreaks) |

**v2 must reproduce these within tolerance before it replaces v1.**

---

## 3. Where it's patchy (the seams v2 fixes)

1. **Stringly-typed domain values.** `manipulation_risk` is a `String` compared `== "low"` across
   `verifier.rs`/`harness.rs`. A typo miscategorizes silently.
2. **Flag text is load-bearing.** `verifier::compute_disagreement_score` infers which dimension
   fired via `flag.contains("emotional_intensity")` etc.; A3's weighting had to match flags by a
   *distinctive substring* because check #6's message lights up three dimension substrings. The
   message string is coupled to the scoring logic.
3. **`deny_unknown_fields` friction.** The A1 `rationale` couldn't live on the telemetry
   sub-structs; it was bolted onto the `ModelProposalOutput` wrapper. Model contract and internal
   enrichment are tangled.
4. **Mega-`Config`.** Every knob = 4 coordinated edits (`Config` + `FileConfig` + `build_config` +
   `validate_config`), and 4 duplicated `make_config` test helpers.
5. **Long imperative `analyze()`** doing six concerns inline; `verify()` accreting positional
   params (`temperature`, then `stop_and_ask_threshold`).
6. **Maintenance tax:** ~10 hand-written `HarnessResult` demo literals in `main.rs` that must be
   updated for any struct change.

None of these are in the *logic* — they're in the wiring. Hence: re-architect the seams, keep the
tuned logic.

---

## 4. v2 target architecture

### 4.1 Cargo workspace (replaces the single crate)

| Crate | Holds | Migration |
|---|---|---|
| `sbh-core` | typed model + `Stage` trait + `Pipeline` + `PipelineCtx`. No I/O. | new |
| `sbh-normalize` | the 7-pass normalizer + tables | **move as-is** |
| `sbh-verify` | checks (as `CheckOutcome`), disagreement scoring, weights, arbitrator, calibration | port |
| `sbh-llm` | `InferenceEngine` trait + 4 backends | **move as-is** |
| `sbh-store` | JSONL stores + `fingerprint`/`iso_now` | **move as-is** |
| `sbh-forge` | forge + sandbox + reputation | **move as-is** |
| `sbh-serve` | proxy + metrics (feature-gated) | **move** |
| `sbh-cli` | the binary; cleaned arg model (clap deferred — dep posture) | port |

Clean seams already exist along these boundaries, so this is mostly `crate::` → `sbh_x::` path
work plus splitting `types.rs`/`harness.rs`.

### 4.2 Model fixes (unblock the advanced tier)

```rust
// Typed domain values
enum Risk { Low, Medium, High }              // was String "low"/"medium"/"high"

// Structured check results — kills the substring coupling + flag_weight() hack
struct CheckOutcome { id: CheckId, dimension: Dimension, weight: f32, fired: bool, detail: String }

// Contract (strict, what the LLM emits) vs Telemetry (internal, extensible)
struct ModelContract { affective, intent, cognitive, rationale?, tool_risk? }  // strict parse
struct Telemetry     { … enrichment attaches here without deny_unknown_fields pain … }
```

### 4.3 The `Stage` seam (makes the advanced tier drop-in)

```rust
#[async_trait] trait Stage { async fn run(&self, ctx: &mut PipelineCtx) -> Result<()>; }
```

`analyze()` becomes an ordered pipeline:

```
Normalize → Propose → Verify → Arbitrate → Calibrate → Formal → Advocate
```

Each stage is independently testable, records its own **duration** (→ observability) and trace
entries. The v1.5 refinement loop, `arbitrator`, and `calibration` become stages. **Formal (F)**
and **Advocate (E)** are both **implemented** and run after the reconcile loop finalizes (so they
see the chosen iteration's telemetry + tool surface and can escalate the final gate):

- **Formal (F)** (`stage_formal`): deterministic predicate engine; no-op unless `formal_rules_path`
  is configured.
- **Advocate (E)** (`stage_advocate`): one gated adversarial LLM pass; no-op unless `advocate_mode`
  is enabled and (for `high_stakes`) the deterministic gate fires. **Raise-only**: it can add
  caution but never clear it.

### 4.4 Layered config

Single source of truth with serde defaults; a generic **Defaults → File → Env** overlay replaces
the `Config`/`FileConfig`/`build_config`/`validate` quartet. One test-config builder replaces the
four duplicated `make_config`s.

---

## 5. Advanced tier (built on the clean core)

| Phase | Feature | Notes / guardrail |
|---|---|---|
| **B** | Observability + timing + `sbh visualize` (HTML/SVG artifact) | **first** — instrument panel; generalize the `serve.rs` Prometheus renderer; **aggregates keyed by fingerprint**, no identifiable per-user series |
| **C** | Tool-aware telemetry (`ToolRisk`) | classify **deterministically first** vs real `capability_request`; LLM only for the fuzz — never trust model self-report as ground truth |
| **D** | HITL weight-tuning (`sbh tune-weights`) | extends A5 store; **advisory → reviewed → applied**, never live; poisoning-guarded |
| **E** ✅ *(shipped, E.1)* | Debate / Devil's Advocate (`src/advocate.rs`) | gated adversarial LLM pass (`advocate_mode` = off/high_stakes/always, `SBH_ADVOCATE`); reuses `run_reconcile` template + a `[ADVOCATE_SYSTEM_PROMPT]` soul section; deterministic high-stakes gate (risk / tool surface / capability request); **raise-only** guardrail (can't self-whitelist); dissent → flag + stop_and_ask + `advocate:dissent` fired-check; transient failure is advisory (not fail-closed); `sbh_advocate_dissent_total` metric. **E.2 ✅ shipped:** `MeteredEngine` wraps the engine per-request — counts every `generate()` and enforces `max_llm_calls_per_request` (`SBH_MAX_LLM_CALLS`, unset = unlimited); optional advocate is skipped when budget's spent; `HarnessResult.llm_calls` + `sbh_llm_calls_total` metric |
| **F** ✅ *(shipped)* | Formal-ish verification (`sbh formal-check`) | deterministic facts-extractor + TOML predicate engine (`src/formal.rs`); `formal_rules_path` config, `SBH_FORMAL_RULES` env; no-op unless configured; high-severity violations force stop_and_ask; fails **closed** on bad rule files; starter domain `rules/credential-egress.toml` |
| **G** *(reserved)* | Meta-cognitive analyzer | **offline `sbh introspect` only**; self-modifying runtime mode is a no-go |

### Cross-cutting decisions (surfaced, not buried)
- **Embedding stack** is a one-time isolated fork (unlocks HITL embedding fine-tuning + the v2
  memory hemisphere) — local `candle`/`ort` vs remote vs stay-keyword. Decide deliberately; don't
  let it enter via a single feature.
- **Global LLM-call ceiling** + per-stage gating is a system setting, watched via B.

---

## 6. Migration guardrails

- **Benchmark parity is the go/no-go gate** (§2.4). No parity → v2 does not ship; v1.5 stays tagged.
- **Move crates one at a time**, tests green after each; v1.5 remains shippable throughout.
- **Timebox each phase.** If a new abstraction fights *worse* than the old, stop and keep evolving
  v1.5 — the honest exit ramp.
- **Never live-apply self-modification** (D, G): advisory → human review → apply, behind the witness layer.
- **Preserve verbatim:** `normalizer` + tables, `soul.md`, `context/default.toml`, `fixtures/` +
  numbers, `security.rs`, forge, `serve.rs` hardening, and the test suite (the migration safety net).
