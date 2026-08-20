# Dual-model split-brain study

**Status: method fixed, arms not yet run. No results in this document are final.**

## The question

SBH's pipeline has two hemispheres: a **proposer** that produces telemetry and a
**verifier** that checks it. Until now both ran on the same model, because there
was no way to configure anything else. `VerifyMode::Reconcile` cited ReConcile
(ACL) — *"diverse models reach consensus"* — while calling one model three times.

So: **does running the two hemispheres on two different models change detection
behaviour, and by how much?**

## Why the published numbers are not the control

The benchmark tables in `README.md` and `TECHNICAL_BRIEF.md` were produced at
`verify_mode = deterministic`. In that mode the verifier hemisphere makes **no LLM
call at all** — it runs eight deterministic consistency checks and stops.

Comparing a cross-model run against those numbers would move two variables at
once: whether a second LLM call happens, and whether the two calls use different
models. The control has to be a **same-model run with `SBH_VERIFY=llm`**.

## Arms

All arms are graded on `fixtures/dualmodel_sample.jsonl` — one fixed, shuffled,
label-balanced sample (200 rows: 100 injection / 100 benign; half from CyberEC
encoding-evasion, half from Deepset natural-language injection), drawn by
`scripts/make_sample.py` with a recorded seed.

| Arm | Proposer | Verifier | `SBH_VERIFY` | Role |
|---|---|---|---|---|
| A | llama3.2:3b | *(none)* | `deterministic` | Published baseline, already on disk. Reference only. |
| B | llama3.2:3b | llama3.2:3b | `llm` | **Control.** Isolates the cost of a second LLM call. |
| C | llama3.2:3b | qwen3.5 | `llm` | Cross-model. |
| D | qwen3.5 | llama3.2:3b | `llm` | Cross-model, reversed. |

**C − B is the dual-model effect.** B − A is the cost of the second call. D exists
because an effect that appears in only one direction is a finding, not noise.

`SBH_REFINE_ITERS=1` is pinned across B/C/D, so every row is exactly one propose
call and one verify call. Refinement otherwise fires a variable number of extra
calls depending on which flags trip — which would make arms differ in cost and
behaviour for reasons that have nothing to do with which model verified.

Positive class and gate are unchanged from the existing benches: positive = any
non-benign label, SBH-positive = `manipulation_risk` in {medium, high}.

## Statistics

The arms are **paired** — the same 200 inputs graded three times — so the test is
McNemar's exact test on the discordant pairs, not a comparison of two headline
rates. `scripts/compare_arms.py` reports it and flags when there are too few
discordant pairs for the test to have power, so an inconclusive result is not
read as evidence of no effect.

A smoke test on synthetic arms is worth knowing about up front: a **10-point**
accuracy gap over 120 independent rows still lands at p ≈ 0.12. Paired arms share
most of their rows and so have more power than that, but if the real effect is
small, 200 rows will not settle it and the honest reporting is "inconclusive at
this n" plus what a sufficient n would be.

## Parse failures shrink the paired set — plan for it

A row that returns non-JSON produces no verdict, and the runner records it as an
`ERROR` rather than scoring it. In the original single-model run this was not
rare: **33% of the rows drawn into this sample errored** (28% of the CyberEC half,
38% of the Deepset half).

That matters more than a 33% loss suggests, because a row must succeed in **every**
arm to be usable as a paired observation. If the same hard rows fail in all arms,
~134 of 200 survive. If failures are independent across arms, ~60 do. Reality sits
between, and it is only knowable after the run.

Two consequences, both deliberate:

- **The sample is over-drawn rather than pre-filtered.** Screening to rows that
  parsed cleanly last time would bias toward inputs llama3.2:3b happens to handle,
  and would not transfer to arm D, where qwen3.5 is the proposer.
- **The per-arm error rate is reported as a result, not as bookkeeping.** "Does
  the proposer model change how often telemetry parses at all?" is a real finding,
  and burying it would overstate whichever arm failed more often.

## Environment

Single box, CPU-only inference (no GPU — Ollama reports `size_vram 0`), Ollama
`llama3.2:3b` (2.0 GB) and `qwen3.5` (6.6 GB), air-gapped, no cloud API involved.
Measured single-call latency on short prompts: llama3.2:3b ≈ 7 s, qwen3.5 ≈ 39 s.
Full-prompt per-row cost is measured by `scripts/probe_arms.py`; see Results.

## Reproducing

```bash
ollama serve                                   # models under /usr/share/ollama
cargo build --release
python3 scripts/make_sample.py --per-class 50  # regenerates the committed sample
./scripts/run_arms.sh                          # arms B, C, D  (--resume to continue)
python3 scripts/compare_arms.py fixtures/dualmodel_arm{B,C,D}.jsonl
```

## Results

*Pending — arms not yet run.*

## Limits

Whatever the outcome, this study establishes something narrow: two specific
3B/7B-class local models, one box, one CPU, ~200 rows, two public datasets. It
does not establish that model diversity helps in general, and it should not be
described that way.
