# Changelog

All notable changes to split-brain-harness are documented here.

---

## [1.5.0] — 2026-09-24

Encoded injections now reach the model decoded: ROT13, and base64 that reads as prose,
not only base64 that happens to contain a keyword. The Reconcile path's verdict and gate
are live. Default-mode (`deterministic`) scoring is unchanged except through the
normalizer.

### Added

**Normalizer: ROT13 decode, and base64 decoded whenever it reads as prose**
- A local red-team run found two encodings reaching Stage 1 still encoded. Bare base64
  was decoded only when the result hit `INJECTION_KEYWORDS`, so a keyword-free payload
  stayed opaque. ROT13 had no pass at all.
- New ROT13 pass: a segment (split at sentence punctuation and colons) is rotated only
  when rotation turns it into language, judged by English/German function words. One
  `rot13` detection per input.
- Bare base64 that decodes to readable prose is now decoded as the new `base64-text`
  kind, including UTF-8 (a German payload with "über" was being skipped). The keyword
  `base64` path is unchanged.
- Both new kinds weigh 0.30, like leetspeak: the input is noted as not passed, but the
  decoded content goes to the model, and the encoding alone never forces `stop_and_ask`.
- Measured on 32 encoded prompts (llama3.2:3b, neutral wrapper, only the normalizer
  differing): attacks caught 9/16 → 12/16, benign flagged 6/16 → 3/16. Small sample;
  direction, not an effect size.

### Measured

**The three-call Reconcile path is inert — measured without a single new LLM call**
- v1.4.0 shipped the dual-model study with an open caveat: every arm ran at
  `VerifyMode::Llm`, so the three-call `VerifyMode::Reconcile` path was never measured.
  That caveat is now discharged, on two independent counts.
- **Reachability.** The adjudicator gate
  (`injection_fingerprint || flag_density >= 0.5`) is recoverable from artifacts
  already on disk, because `CheckOutcome::fired()` is defined as `detail.is_some()` and
  `consistency_flags` records every `detail` — the recorded flag set *is* the fired set.
  Over all 1,002 study rows the gate opens **0 times**. The urgency-with-low-risk half
  of the fingerprint never fires at all: these models report high risk whenever they
  report urgency (the corpus holds 32 rows of exactly the opposite pattern).
- **Effect.** Even when the gate does open, the verdict changes nothing.
  `confidence` was copied out of `disagreement` before the reconcile block, and
  `stop_and_ask` / `passed` derived from that copy, while `reconcile_verdict` had no
  reader in production code at all. **This half is now fixed — see Changed below.**
- `scripts/reconcile_gate.py` reproduces the gate numbers from any bench artifact.

### Changed

**The adjudicator verdict now feeds the decision — escalate-only**
- `VerifyMode::Reconcile` wires the adjudicator's verdict into `stop_and_ask` under a
  ratchet: **`injection` escalates; `benign`, `ambiguous`, an unrecognised string, a
  parse failure and an unreachable adjudicator all change nothing.**
- The asymmetry is a security argument, not a statistical one. The gate opens only on
  rows already judged suspicious, and the payload that raised those flags is in the
  adjudicator's own prompt — so a `benign` verdict is the suspect input arguing its own
  case to a third model. Honouring it would let a payload that talks its way past one
  call clear the flags raised against it. A wrong `injection` costs one unnecessary
  `stop_and_ask` on an already-flagged row; the payoffs are not symmetric, so neither
  is the code.
- Verdict matching normalises case and whitespace: safety must not hinge on a model
  shouting.
- `passed` needed no wiring — the gate implies at least two fired checks, so `passed`
  is already `false` whenever the adjudicator runs.
- **No published benchmark is affected.** Every benchmark in this repo ran at
  `deterministic` or `llm` verify mode, and `llm_mode_never_escalates` pins that
  neither consults an adjudicator.
- Seven tests cover the semantics, including a baseline test asserting the scenario
  does not already stop — without which the escalation test would pass vacuously.
- Wiring alone did not make the path measurable — the gate still had to be fixed, which
  the next entry does.

**The Reconcile gate has a reachable trigger**
- The old trigger (`injection_fingerprint || flag_density >= 0.5`) opened **0 times in
  1,002 rows**. Both disjuncts asked for things these models do not produce: the
  fingerprint wanted adversarial tone AND high urgency AND an asserted low risk (they
  report high risk whenever they report urgency), and the density wanted four of eight
  checks when the observed per-row maximum is two.
- The trigger is now **"a check about intent fired"** (`Dimension::is_intent_signal`).
  Coherence and RiskValue are excluded as quality signals — a third opinion on garbled
  text has nothing to work with. Opens on **78 of 957 rows (8.2%)**.
- The split was scored against labels, not tuned until it fired
  (`scripts/gate_candidates.py`): it reaches **34 of the 243 missed injections (14.0%)**
  at a cost of **zero** correctly-passed benign rows. Including Coherence would reach 5
  more misses and cost 3 false escalations — **the only benign rows in the corpus that
  fire any check fire Coherence, and nothing else.**
- Both old disjuncts are kept as a floor, though
  `old_gate_conditions_are_subsumed_by_the_intent_signal` proves them redundant across
  all 256 dimension subsets.
- **Stated before the run:** 14.0% is a ceiling, not an expectation — the adjudicator
  must actually return `injection` to convert a row. The deeper limit is not the gate:
  the proposer's telemetry raises no flag at all on 84% of the injections it misses.

### Fixed

**`scripts/reconcile_gate.py` counted a flag the gate cannot see**
- `stage_obfuscation` inserts an "obfuscation detected" string into `consistency_flags`
  *after* `verify()` returns, so it is in every artifact but was never visible to the
  gate. The first version of the script counted it toward `flag_density`.
- The conclusion was unaffected (0 either way, and correct counting puts the old gate
  further from firing — the real per-row maximum is 2 fired checks, not 3), but the
  script measured the wrong quantity.
- `run_bench_labeled.py` now records `fired_checks`, so gate analysis need never match
  on flag text again.

**Leetspeak pass no longer rewrites identifiers with a digit suffix**
- A word whose leet characters are all one trailing run is left alone: `ROT13`,
  `sha256`, `win32`, `Python3`, and emphasis like `Hilfe!!` (`!` is in the leet map).
  Before, `ROT13` reached the model as `ROTie` and scored as obfuscation (0.46) on its
  own. Leet substitutes *inside* words (`h4x0r`, `1gn0r3`, `h4ck3r5`), and that is still
  decoded; all 8 leetspeak attacks from the red-team set are still detected.

**Leetspeak pass no longer flattens whitespace**
- Any leet rewrite rebuilt the whole input with `split_whitespace().join(" ")`, so
  newlines, blank lines, tabs and runs of spaces all collapsed to single spaces before
  Stage 1. Words are now rewritten in place and the original whitespace is kept.

**Demo sends the bearer token (#21)**
- `sbh demo --serve` now actually sends the configured bearer token on its chat
  requests, so the live demo works against a key-protected `sbh serve`.

**LICENSE names the right entity (#20)**
- The copyright holder is SGAIL LLC, not "SGAIL, Inc.".

### Packaging

- **`sbh-normalize` 0.2.0.** `DetectionKind` gains `Base64Text` and `Rot13`. That is a
  breaking change for anyone matching the enum exhaustively, so it takes a minor bump
  under 0.x. Publish it before the root crate, which now requires `0.2.0`. The other
  member crates are unchanged at 0.1.0.

---

## [1.4.0] — 2026-08-26

First release to reach crates.io since 1.2.0, and the first that ships the
dual-model study's findings.

### Packaging

**The workspace is publishable to crates.io**
- Every `sbh-*` path dependency now carries a `version`, which is what `cargo publish`
  requires; `cargo publish --workspace` packages all seven crates in dependency order.
  crates.io has been stuck at the pre-workspace v1.2.0 since the v2 clean-core split.
- The six member crates (`sbh-core`, `sbh-normalize`, `sbh-llm`, `sbh-safety`,
  `sbh-store`, `sbh-forge`) enter at `0.1.0` and carry `repository` metadata.
- The root package now `exclude`s the benchmark corpora and run logs: 713 KiB packaged
  (185 KiB compressed), down from ~10 MB. `fixtures/eval.json` stays in — `tests/eval.rs`
  reads it. The excluded data remains in git, where the study evidence belongs.

### Added

**Per-role models — the two hemispheres can run on different models**
- `SBH_VERIFIER_MODEL` / `SBH_ADJUDICATOR_MODEL` (config: `verifier_model_name`,
  `adjudicator_model_name`) route the verifier hemisphere and the Reconcile
  adjudicator to models of their own. Unset — the default — means every role uses
  `model_name`, builds exactly one backend client, and behaves exactly as before.
- The per-request LLM-call ceiling is shared across roles: a split-brain run gets
  the same budget as a single-model one, not one per engine.
- Results carry a `models` block (`proposer` / `verifier` / `adjudicator` /
  `verify_mode` / `temperature` / `split`) so an artifact records what produced it.
  It reports the engines actually wired, not the config — an override with no
  engine set behind it is not reported as a split.
- Study tooling: `scripts/make_sample.py` (fixed label-balanced sample),
  `scripts/run_arms.sh` (arms B/C/D), `scripts/probe_arms.py` (wall-clock probe),
  and `scripts/compare_arms.py` (paired metrics + McNemar's exact test).
  `run_bench_labeled.py` records per-row provenance and an `--arm` label.

### Note

Prior benchmark tables in this repo are single-model runs at
`verify_mode = deterministic` — the verifier hemisphere made no LLM call at all.
They are not a control for a dual-model comparison; see `docs/DUAL_MODEL_STUDY.md`.

### Yanked

**crates.io `1.3.0` is yanked.** It was uploaded from this commit rather than from
the `v1.3.0` tag, so its contents were 32 commits ahead of the GitHub release of the
same name — it contained per-role models and the corrected benchmark tables, which
the tagged v1.3.0 source does not. Rather than leave two different trees sharing a
version number, that upload is yanked and this release carries the code instead.
Yanking does not break existing lockfiles. The GitHub `v1.3.0` tag is unchanged.

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
