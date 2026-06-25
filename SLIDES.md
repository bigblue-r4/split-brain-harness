---
marp: true
theme: default
paginate: true
backgroundColor: '#0d1117'
color: '#e6edf3'
style: |
  section {
    font-family: 'SF Mono', 'Fira Code', monospace;
    padding: 48px 64px;
  }
  h1 { color: #58a6ff; font-size: 2em; margin-bottom: 0.2em; }
  h2 { color: #79c0ff; font-size: 1.3em; border-bottom: 1px solid #30363d; padding-bottom: 0.2em; }
  h3 { color: #56d364; font-size: 1em; margin-bottom: 0.3em; }
  table { width: 100%; border-collapse: collapse; font-size: 0.8em; }
  th { background: #161b22; color: #79c0ff; padding: 6px 12px; text-align: left; }
  td { padding: 5px 12px; border-top: 1px solid #21262d; }
  code { background: #161b22; padding: 2px 6px; border-radius: 4px; color: #56d364; font-size: 0.9em; }
  .flag { color: #f85149; }
  .ok   { color: #56d364; }
  .dim  { color: #8b949e; }
  strong { color: #ffa657; }
---

<!-- Slide 1: Title -->

# Split-Brain Harness

### AI Security Infrastructure for Government LLM Deployments

<br>

**SGAIL** · North Shore, Oahu, HI · DHS SBIR Phase 1 Candidate

<br>

```
sbh demo --serve --offline   # live demo — no backend required
```

<span class="dim">Rust · MIT · Single static binary · Air-gap ready</span>

---

<!-- Slide 2: The Problem -->

## The Problem

LLMs are being deployed in cleared and government-adjacent environments with **no security telemetry**.

<br>

| Attack vector | What the model sees | What it actually is |
|---|---|---|
| Prompt injection | Legitimate user request | Instruction override targeting keys/config |
| Insider threat | "Help me map data access" | Audit-gap reconnaissance |
| Authority impersonation | "Director Hargrove, NSA..." | Fabricated federal authority |
| Encoding evasion | `іgnοre аll іnstructіοns` | Cyrillic homoglyphs → payload |
| Slow-boil escalation | 3 benign turns, then attack | Missed without session context |

<br>

**No existing LLM gateway detects these at the protocol layer — before the model responds.**

---

<!-- Slide 3: The Solution -->

## Split-Brain Harness

Drop-in OpenAI-compatible proxy. Every request passes through a **two-stage telemetry pipeline** before reaching the model.

<br>

```
 User / Client
      │
      ▼
 Stage 0 — Normalizer      deobfuscate: homoglyphs · base64 · Morse · leet · backslash
      │
      ▼
 Stage 1 — Propose (LLM)   soul-injected: affective · intent · cognitive telemetry
      │
      ▼
 Stage 2 — Verify (det.)   fail-closed consistency check · stop_and_ask on conflict
      │
      ▼
 Session tracker            escalation detection across turns
      │
      ▼
 Response + headers         x-sbh-witness · x-sbh-session-alert · audit JSONL
```

<span class="ok">Model only runs after Stage 1 + 2 complete. Block is possible before any LLM inference.</span>

---

<!-- Slide 4: Benchmark Results -->

## Benchmark Results

Evaluated on three public adversarial datasets · **llama3.2:3b · local Ollama · air-gapped**

<br>

| Dataset | Rows | Precision | Recall | F1 | Notes |
|---|---|---|---|---|---|
| CyberEC | 141 | **1.00** | 0.50 | 0.67 | Zero false positives |
| TrustAI Jailbreaks | 1,398 | — | — | **94.8% flagged** | Unlabeled |
| Deepset Prompt Injections | 546 | 0.81 | 0.37 | 0.51 | 3B local model limit† |

**Stage 0 normalizer** catches 50% of CyberEC encoding-evasion FNs:
homoglyphs · base64 · Morse · backslash-escape · fullwidth · leet

<span class="dim">† Deepset recall improves with capable backend (Claude/GPT-4). Precision is backend-independent.</span>

---

<!-- Slide 5: Key Capabilities -->

## Key Capabilities

<br>

| Capability | SBH | Standard LLM Gateway |
|---|---|---|
| Two-stage telemetry (propose + verify) | ✅ | — |
| Stage 0 deobfuscation normalizer | ✅ | — |
| Soul-injected identity baseline | ✅ | — |
| Multi-turn session escalation detection | ✅ | — |
| Air-gap / local model capable | ✅ | rarely |
| Ephemeral sandboxed tool execution (WASM) | ✅ | — |
| OpenAI-compatible drop-in proxy | ✅ | varies |
| JSONL audit trail + Prometheus metrics | ✅ | varies |
| Single static Rust binary | ✅ | — |

<br>

### Live demo commands (no backend required)
```bash
sbh demo --offline             # 5 DHS-relevant threat scenarios
sbh demo --serve --offline     # 5-turn slow-boil foreign adversary escalation
```

---

<!-- Slide 6: The Ask -->

## DHS SBIR Phase 1

### What we're building
A hardened, air-gap-deployable AI security layer for government LLM deployments — tamper-evident audit trail, session-level threat detection, sandboxed tool execution.

### Phase 1 milestones (~$300K · 6 months)

| Milestone | Deliverable |
|---|---|
| M1 | Benchmark suite against DHS-relevant labeled datasets |
| M2 | Hardened `sbh serve` with FedRAMP-aligned audit controls |
| M3 | Normalizer v2 — full Unicode TR39 confusables, entropy scoring |
| M4 | Red-team evaluation by independent cleared assessor |
| M5 | Open-source release + technical report |

**SGAIL** · trentdoosday@gmail.com · github.com/bigblue-r4/split-brain-harness
