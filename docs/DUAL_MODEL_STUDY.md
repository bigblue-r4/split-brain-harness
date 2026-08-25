# Dual-model split-brain study

**Status: all four arms complete (2026-08-25). Results below.**

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
| E | qwen3.5 | qwen3.5 | `llm` | **Second control.** Same-model control for D. |

**C − B is the dual-model effect.** B − A is the cost of the second call. D exists
because an effect that appears in only one direction is a finding, not noise.

**Why E was added.** Arm D moves two variables at once against B — the proposer
model changes *and* the hemispheres start to differ. So if D beats B, "qwen3.5 is
simply the better proposer" accounts for the entire gain without model diversity
contributing anything, and there is no arm on disk that can tell the two apart.
Arm E runs qwen3.5 on both hemispheres, which makes **D − E the dual-model effect
at qwen3.5's proposer** — the mirror of C − B at llama3.2's — and **E − B the
proposer-model effect** with diversity held out of it.

E was added on 2026-08-24, after B and C had finished and D had shown a
significant gain over B on both escalation metrics. It is a control for a
confound the data exposed, not a new hypothesis, and it does not replace the
pre-registered comparison: **C − B remains the primary test.** D − E is
secondary and should be reported as such.

`SBH_REFINE_ITERS=1` is pinned across B/C/D/E, so every row is exactly one propose
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

**Read that 33% with care.** `run_bench_labeled.py` carried a hardcoded 180-second
per-row subprocess timeout, written when a row was a single LLM call. A
`verify_mode=llm` row makes two, and rows on this hardware routinely take 190s+ —
so the first launch of this study produced an all-ERROR arm B, every row failing
on the stopwatch rather than on anything the model did. The timeout is now
`SBH_ROW_TIMEOUT` (default 1800s). The same ceiling applied to the historical runs,
so an unknown share of that 33% were timeouts, not parse failures, and the usable
paired set may be larger than the pessimistic estimate below.

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
./scripts/run_arms.sh                          # arms B, C, D, E  (--resume to continue)
python3 scripts/compare_arms.py fixtures/dualmodel_arm{B,C,D,E}.jsonl
```

## Results

**The dual-model hypothesis is not supported.** Neither test of model diversity found
a benefit, and the one that reached significance found a *disadvantage*. The effect
that does exist belongs to the proposer model, not to the pairing.

All four arms ran on the same fixed 200-row sample, same box, same binary
(`target/release/split-brain-harness`, built 2026-08-20), CPU-only.

### Per-arm

| Arm | Proposer / Verifier | n | Parse fail | Escalated | risk-only acc | catch acc | nonanswer acc | median s/row |
|---|---|---|---|---|---|---|---|---|
| B (control) | llama3.2:3b / llama3.2:3b | 185 | 15 | 67 (36%) | 0.773 | 0.838 | 0.616 | 160.7 |
| C | llama3.2:3b / qwen3.5 | 186 | 14 | 54 (29%) | 0.790 | 0.860 | 0.624 | 114.5 |
| D | qwen3.5 / llama3.2:3b | 200 | 0 | 44 (22%) | 0.835 | 0.900 | 0.730 | 247.5 |
| E | qwen3.5 / qwen3.5 | 200 | 0 | 20 (10%) | 0.830 | 0.880 | 0.780 | 370.1 |

Precision under every scoring stayed between 0.841 and 1.000; arm E was 1.000 on all
three. The arms differ in recall and in escalation behaviour, not in false alarms.

### The two tests of diversity

**C − B (pre-registered, at llama3.2:3b's proposer): inconclusive.** +0.022 risk-only
(p=0.29), +0.033 escalation-as-catch (p=0.18), +0.011 escalation-as-nonanswer (p=0.80),
on 8 to 16 discordant pairs. Consistently positive in sign and nowhere near
significance. As the Statistics section commits to: **this is inconclusive at this n,
not evidence of no effect.** Settling an effect of this size needs roughly 340–420
paired rows, against the 184 that survived.

**D − E (at qwen3.5's proposer): no benefit, and a significant disadvantage on one
metric.** Both arms completed 200/200 with zero parse failures, so this is the
best-powered comparison in the study — every row is paired.

| Metric | D (diverse) | E (same-model) | Difference | McNemar |
|---|---|---|---|---|
| risk-only | 0.835 | 0.830 | −0.005 | p=1.00 (4/3 discordant — no power) |
| escalation-as-catch | 0.900 | 0.880 | −0.020 | p=0.42 |
| escalation-as-nonanswer | 0.730 | 0.780 | **+0.050 favouring E** | **p=0.021** |

On the only metric where the two arms significantly differ, **the same-model arm wins**.

Taken together: two independent tests, neither supporting diversity, one significantly
against it. That is enough to stop describing this pipeline as benefiting from diverse
models — a claim `VerifyMode::Reconcile` inherited from its ReConcile citation and which
this study was built to check. It is **not** enough to claim diversity is harmful in
general; D − E is one significant metric on one pairing on one box.

### What the effect actually is: the proposer

Swapping the proposer from llama3.2:3b to qwen3.5 moves the numbers; swapping the
verifier does not.

| Comparison | escalation-as-nonanswer | McNemar |
|---|---|---|
| E − B (proposer changed, both same-model) | +0.162 | p<0.0001 |
| D − B (proposer changed, diversity added) | +0.141 | p<0.0001 |
| C − B (verifier changed only) | +0.011 | p=0.80 |

The proposer-only change (E − B) is the **largest** effect in the study — larger than
the arm that also added diversity.

**State this narrowly.** E − B is significant on escalation-as-nonanswer only. On
risk-only (+0.059, p=0.071) and escalation-as-catch (+0.049, p=0.122) it does not reach
significance, and the same is true of D − B. The supported claim is: *a qwen3.5 proposer
produces explicit medium/high risk labels on substantially more adversarial inputs, and
produces parseable telemetry more reliably.* Anything broader is not in this data.

### Parse failures split cleanly along the proposer

Both llama3.2:3b-proposing arms failed to parse on 15 and 14 of 200 rows. Both
qwen3.5-proposing arms failed on **zero**. Which model proposes decides whether
telemetry parses at all — with no exceptions in 800 rows.

This also retires the 33% figure this document warned about. With the row timeout
raised from the inherited 180s to 1800s, the true rate for llama3.2:3b is ~7.5%; most
of the original 33% was rows being killed on the stopwatch, exactly as suspected.

### Mechanism: labelling instead of deferring

Escalation rate falls monotonically as qwen3.5 takes over hemispheres — B 36%, C 29%,
D 22%, E 10% — while escalation-as-nonanswer recall rises from 0.174 (B) to 0.560 (E).

So the better arms are not catching more by escalating more. They escalate *less* and
commit to an explicit risk label more often. This is why the two scoring conventions
disagree so sharply about arm E: the reading that credits an escalation as a catch sees
+0.049 over the control, while the reading that credits only an explicit medium/high
label sees +0.162. Reporting one convention alone would have hidden the actual change in
behaviour.

### Cost

The honest answer is expensive. Arm E is **2.3x slower per row than the control** —
370.1s vs 160.7s median, 21.8h vs 7.2h for 200 rows — on CPU-only hardware. For an edge
deployment that is the operative finding, not a footnote: the accuracy gain is bought
entirely with latency.

One anomaly is left unexplained rather than smoothed over: arm C's median (114.5s) is
*lower* than the control's (160.7s) despite adding a slower model as verifier. Both runs
were on the same box with the same pinned settings; the likely cause is background load
differing between runs, since wall-clock latency here is not a controlled variable. It
should not be read as "adding qwen3.5 as verifier makes the pipeline faster."

## Limits

Whatever the outcome, this study establishes something narrow: two specific
3B/7B-class local models, one box, one CPU, ~200 rows, two public datasets. It
does not establish that model diversity helps in general, and it should not be
described that way.

That caution was written expecting a positive result, and it binds just as hard
now that the result is negative. **This study does not establish that model
diversity fails in general either.** What it shows is that on this pairing, this
sample and this hardware, diversity produced no measurable benefit and one
significant disadvantage — enough to withdraw the claim the code was making, not
enough to make the opposite claim. Two 3B/7B-class local models are also a weak
test of diversity: they may simply be too similar, or too weak, for consensus
between them to mean anything. A pairing with genuinely different training
lineages could behave differently, and this design would not have detected it.

The proposer finding carries its own limit: it rests on one scoring convention
(escalation-as-nonanswer) and the parse rate. The other two conventions move in
the same direction without reaching significance, which is suggestive and nothing
more.
