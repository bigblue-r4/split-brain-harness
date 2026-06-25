# Split-Brain Harness — Technical Brief

**SGAIL Labs** · North Shore, Oahu, HI · trentdoosday@gmail.com  
Version: 2.x · June 2026 · DHS SBIR Phase 1 Candidate

---

## What It Does

Split-Brain Harness (SBH) is an open-source Rust framework that wraps any LLM with a
two-stage telemetry pipeline. Every request is analyzed for affective, intent, and
cognitive threat signals before reaching the model — and again, deterministically, before
the response is returned. It runs as a drop-in OpenAI-compatible proxy with no changes
to the downstream application.

---

## Pipeline Architecture

```
User / Client
     │
     ▼
┌────────────────────────────────────────────┐
│  SBH Serve  (OpenAI-compatible HTTP proxy) │
│  POST /v1/chat/completions                 │
└────────────┬───────────────────────────────┘
             │
             ▼
┌────────────────────────────────────────────┐
│  Stage 1 — Propose  (soul-injected LLM)   │
│                                            │
│  Soul file loaded as system context.       │
│  Adaptor layer injects trigger-matched     │
│  context packs (transformer RAG).          │
│                                            │
│  Output: TelemetryResult JSON              │
│    • affective_telemetry                   │
│        primary_emotion, emotional_intensity │
│        structural_tone[]                   │
│    • intent_matrix                         │
│        stated_objective                    │
│        subtextual_motive                   │
│        manipulation_risk  (low/med/high)   │
│    • cognitive_state                       │
│        urgency_vector  [0.0–1.0]           │
│        coherence_rating [0.0–1.0]          │
└────────────┬───────────────────────────────┘
             │
             ▼
┌────────────────────────────────────────────┐
│  Stage 2 — Verify  (deterministic)        │
│                                            │
│  Cross-checks telemetry for internal       │
│  consistency. Fail-closed: any              │
│  inconsistency is a flag, not a warning.   │
│                                            │
│  Output: VerificationReport                │
│    • passed: bool                          │
│    • consistency_flags[]                   │
│    • confidence  [0.0–1.0]                 │
│    • stop_and_ask: bool                    │
└────────────┬───────────────────────────────┘
             │
             ▼
┌────────────────────────────────────────────┐
│  Response enrichment                       │
│  x-sbh-telemetry: {risk, emotion, ...}     │
│  x-sbh-session-alert: escalation_detected  │
│  x-sbh-witness: passed|flagged             │
└────────────────────────────────────────────┘
```

The model only runs after Stage 1 and 2 both complete. If the verification stage flags
the request with `stop_and_ask: true`, the proxy can be configured to block before
ever reaching the downstream LLM.

---

## Threat Taxonomy

SBH is calibrated against four threat categories relevant to government and cleared
contractor AI deployments:

| Category | Signal Pattern | Key Indicators |
|---|---|---|
| **Prompt Injection** | Instruction override attempt | Directive tone, "ignore all previous instructions," exfiltration target (keys, config) |
| **Insider Threat** | Access scoping, audit evasion | Casual tone masking sensitive data targeting; explicit "without triggering review" framing |
| **Foreign Adversary / Impersonation** | Fabricated federal authority | Unverifiable authority claim in-channel; personnel clearance data as target; urgency to bypass verification |
| **BEC via AI Proxy** | Financial coercion | urgency_vector > 0.9, coherence < 0.3, "do not log" explicit evasion, off-channel approval chain |

---

## Session Escalation Detection

For multi-turn deployments (AI assistants, helpdesk bots, procurement systems), SBH
tracks per-session risk trajectories. Escalation fires when:

1. At least 3 turns have accumulated
2. The latest risk score ≥ medium (score ≥ 1.0)
3. Latest score exceeds the historical session mean by > 0.5

When escalation fires, the proxy returns `x-sbh-session-alert: escalation_detected`
and appends a JSONL entry to the session log (masked IP + input fingerprint — no raw
input stored).

This catches slow-boil attacks: an adversary who opens with benign queries and
gradually escalates is detected at the inflection point, not only on the final overtly
adversarial message.

---

## Benchmark Results

Evaluated against two public labeled adversarial datasets (local Ollama, llama3.2:3b):

| Dataset | Rows | Precision | Recall | Notes |
|---|---|---|---|---|
| Deepset Prompt Injections | 546 | ~0.85 | ~0.72 | FN gap: context-embedded injections (payload in document body, not top-level query) |
| CyberEC | 200 | TBD | TBD | Run complete; analysis in progress |
| TrustAI Jailbreaks | 1,405 | TBD | TBD | Unlabeled; flagging rate analysis in progress |

The Deepset false-negative gap is a known structural blind spot: SBH analyzes the
surface query, not document bodies. Mitigation: chunked document analysis via the
Ephemeral Tool Forge.

---

## Ephemeral Tool Forge

A 5-phase system for safely generating, sandboxing, and reputation-tracking
LLM-produced Rust tools:

1. **Propose** — LLM generates Rust source for the requested capability
2. **Compile** — `rustc` to `wasm32-wasip1`, 60-second hard timeout
3. **Execute** — `wasmtime` sandbox, 15-second execution timeout
4. **Audit** — JSONL entry: capability, FNV-1a-64 source fingerprint, attempt count, tier
5. **Reputation** — tool promoted/demoted across tiers based on successive success/failure

No generated code touches the host OS. No network access inside the sandbox.

---

## Air-Gap Deployment

SBH requires no cloud dependency. Full deployment on a classified or air-gapped network:

```bash
# 1. Install Ollama and pull model
ollama pull llama3.2:3b

# 2. Start SBH proxy
SBH_BACKEND=ollama-native \
SBH_MODEL=llama3.2:3b \
SBH_SERVE_KEY=<bearer-token> \
SBH_AUDIT_PATH=/var/log/sbh/audit.jsonl \
SBH_SESSION_LOG=/var/log/sbh/sessions.jsonl \
sbh serve --listen 0.0.0.0:8088

# 3. Point existing applications at SBH instead of OpenAI
OPENAI_BASE_URL=http://localhost:8088/v1
```

Prometheus metrics at `/metrics` (protected by the same bearer token).  
All log files are JSONL — forward to any SIEM via standard log pipeline.

---

## Key Differentiators

| Feature | SBH | Standard LLM Gateway |
|---|---|---|
| Two-stage telemetry (propose + verify) | ✓ | — |
| Soul-injected identity (unforgeable baseline) | ✓ | — |
| Session escalation detection | ✓ | — |
| Air-gap / local model capable | ✓ | rarely |
| Ephemeral sandboxed tool execution | ✓ | — |
| OpenAI-compatible drop-in proxy | ✓ | varies |
| JSONL audit trail + Prometheus metrics | ✓ | varies |
| Rust, single static binary | ✓ | — |

---

## Project Status

- **Language**: Rust (stable)
- **Tests**: 339 passing, CI green
- **Deployment**: single binary, `cargo install` or pre-built release
- **License**: MIT
- **Repository**: github.com/bigblue-r4/split-brain-harness

Demo command (no backend required):

```bash
sbh demo --offline           # 5 DHS-relevant threat scenarios
sbh demo --serve --offline   # multi-turn slow-boil escalation walkthrough
```
