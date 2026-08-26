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
python3 scripts/bootstrap_ci.py  fixtures/dualmodel_arm{B,C,D,E}.jsonl
./scripts/run_arms.sh B2                       # test-retest replicate of arm B
```

## Results

**The dual-model hypothesis is not supported.** Neither test of model diversity found
a benefit. The effect that does exist belongs to the proposer model, not to the pairing.

All four arms ran on the same fixed 200-row sample, same box, same binary
(`target/release/split-brain-harness`, built 2026-08-20), CPU-only.

### Claim hierarchy

Ordered by how much weight each will bear. Read down until you stop believing it;
everything above that point is still standing.

1. **Parse-failure rate collapses when the stronger model proposes** — 15/200 and
   14/200 for the llama3.2:3b-proposing arms, **0/200 for both** qwen3.5-proposing
   arms. No significance test, no scoring convention, no multiplicity exposure,
   non-overlapping Wilson intervals. The most solid result in the study.
2. **The qwen3.5-proposing arms detect substantially more under
   escalation-as-nonanswer** — +0.141 and +0.162 against the control, both
   p<0.0001, both surviving every multiplicity correction applied below.
3. **No reliable evidence that model diversity improves anything.** The
   pre-registered test (C − B) is inconclusive; the better-powered one (D − E)
   shows no benefit. This is a null result, and it is stated as one.
4. **The apparent D − E advantage for the same-model arm does not survive
   multiplicity correction** and should not be reported as a finding. See below.
5. **Measurement noise is now quantified, and it is low.** Arm B2 re-ran arm B
   unchanged: differences of ~0.011 with symmetric discordance. Points 1 and 2 sit
   an order of magnitude above that floor; point 4's effect does not look like noise
   either, it is simply under-powered. See *The noise floor* below.

### Per-arm

| Arm | Proposer / Verifier | n | Parse fail | Escalated | risk-only acc | catch acc | nonanswer acc | median s/row |
|---|---|---|---|---|---|---|---|---|
| B (control) | llama3.2:3b / llama3.2:3b | 185 | 15 | 67 (36%) | 0.773 | 0.838 | 0.616 | 160.7 |
| C | llama3.2:3b / qwen3.5 | 186 | 14 | 54 (29%) | 0.790 | 0.860 | 0.624 | 114.5 |
| D | qwen3.5 / llama3.2:3b | 200 | 0 | 44 (22%) | 0.835 | 0.900 | 0.730 | 247.5 |
| E | qwen3.5 / qwen3.5 | 200 | 0 | 20 (10%) | 0.830 | 0.880 | 0.780 | 370.1 |

Precision under every scoring stayed between 0.841 and 1.000; arm E was 1.000 on all
three. The arms differ in recall and in escalation behaviour, not in false alarms.

**The n column is not constant, and the attrition is not random.** Arms B and C are
graded only on the rows they could parse (185 and 186); D and E on all 200. The rows B
dropped are presumably the harder ones, so B's row in this table is computed on an
easier subset than E's — reading the table rows against each other flatters the control.
Every paired test below restricts both arms to their shared rows, so the comparisons are
unaffected. Note the direction: this bias runs **in favour of the control**, so the true
proposer effect is more likely understated than overstated here.

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

The same-model arm is nominally ahead on the one metric that separates them — but
**that result does not survive correction for the number of tests run, and is not
claimed here.** Its interval, +0.050 [+0.010, +0.090], only just clears zero.

The defensible reading is the **null**: two independent tests, neither showing a
benefit from diversity. That is enough to stop describing this pipeline as benefiting
from diverse models — a claim `VerifyMode::Reconcile` inherited from its ReConcile
citation and which this study was built to check. It is **not** enough to claim
diversity is harmful.

### Multiplicity

Six pairwise comparisons x three scoring conventions = **18 tests**, reported without
correction above. That inflates the chance of at least one false positive, and the
smallest p-value is the one most likely to be it — so it is worth naming which results
survive and which do not.

The three scoring conventions are three readings of the same rows, not independent
experiments, so Bonferroni across all 18 is conservative. Both thresholds are given:

| Test | p | vs family-of-3 (0.0167) | vs all-18 (0.00278) |
|---|---|---|---|
| B − D escalation-as-nonanswer | 0.000013 | pass | pass |
| B − E escalation-as-nonanswer | 0.000001 | pass | pass |
| C − D escalation-as-nonanswer | 0.000431 | pass | pass |
| C − E escalation-as-nonanswer | 0.000009 | pass | pass |
| **D − E escalation-as-nonanswer** | **0.021271** | **fail** | **fail** |

Every proposer result survives both. The single diversity result **fails both**, which
is why the claim hierarchy above does not rest on it. Settling D − E properly needs
~212 paired rows for the lenient threshold or ~287 for the strict one, against the 200
in hand — close, but not reached.

### Effect sizes

P-values answer "could this be nothing?" and stop. `scripts/bootstrap_ci.py` reports
the paired accuracy difference with a percentile bootstrap 95% CI (10,000 resamples,
seeded, resampling paired rows so the pairing is preserved). Selected:

| Comparison | Difference | 95% CI | Discordant |
|---|---|---|---|
| B → E, escalation-as-nonanswer | +0.162 | [+0.103, +0.222] | 4/34 |
| B → D, escalation-as-nonanswer | +0.141 | [+0.081, +0.200] | 5/31 |
| C → E, escalation-as-nonanswer | +0.156 | [+0.091, +0.220] | 7/36 |
| D → E, escalation-as-nonanswer | +0.050 | [+0.010, +0.090] | 3/13 |
| B → C, risk-only | +0.022 | [−0.005, +0.054] | 2/6 |
| D → E, escalation-as-catch | −0.020 | [−0.055, +0.015] | 9/5 |

Parse-failure rates as Wilson intervals, which stay honest at a rate of zero where the
normal approximation does not: B 0.075 [0.046, 0.120], C 0.070 [0.042, 0.114],
**D and E 0.000 [0.000, 0.019]** — non-overlapping with both llama-proposing arms.

Every comparison not involving escalation-as-nonanswer has a CI crossing zero.

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

### The noise floor (arm B2)

Temperature is 0.1, not 0, so the same configuration does not give the same answer
twice. Until that was measured, every difference in this study was being read against
an unknown. Arm B2 is arm B re-run unchanged — same models, same sample, same binary,
same settings — so every difference between them is noise by construction.

| | Arm B | Arm B2 (replicate) |
|---|---|---|
| Parse failures | 15/200 (0.075) | **14/200 (0.070)** |
| Escalated | 67 (36%) | **69 (37%)** |
| Median s/row | 160.7 | **165.3** |
| risk-only accuracy | 0.773 | 0.780 |
| escalation-as-catch | 0.838 | 0.844 |
| escalation-as-nonanswer | 0.616 | 0.627 |

Paired, on the 185 rows both arms answered:

| Scoring | Difference | 95% CI | Discordant | McNemar |
|---|---|---|---|---|
| risk-only | +0.011 | [−0.022, +0.043] | 4/6 | p=0.75 |
| escalation-as-catch | +0.011 | [−0.032, +0.054] | 7/9 | p=0.80 |
| escalation-as-nonanswer | +0.011 | [−0.022, +0.043] | 4/6 | p=0.75 |

**Two things matter here, and only one of them is the magnitude.**

The size of the noise is small: run-to-run accuracy moves ~0.011, with a 95% band of
roughly ±0.03 to ±0.05. The proposer effects (+0.141, +0.162) sit an order of magnitude
above it. The parse-failure result reproduced almost exactly — 15 then 14 — which is the
strongest confirmation available that it is a property of the model and not of the run.

The *shape* matters more. Noise produced discordance volumes of 10, 16 and 10 pairs —
comparable to the 16 discordant pairs behind D − E's escalation-as-nonanswer result. So
volume alone cannot separate signal from noise here. What separates them is asymmetry:
noise split its discordant pairs **4/6, 7/9, 4/6** — close to even, which is what
chance looks like. D − E split **3/13**, 81% in one direction. Chance does not do that,
and McNemar tests precisely that asymmetry.

So D − E is most likely a real effect that is simply under-powered, exactly as the
sample-size estimate above implies — **not** an artifact of run-to-run variation. It
still does not survive multiplicity correction, and it is still not claimed. Those two
statements are compatible, and keeping them apart is the point of reporting both.

One curiosity, recorded rather than explained: B2 came out +0.011 ahead on all three
scorings, i.e. exactly two more rows correct under each. With 10 to 16 discordant pairs
per scoring that is coincidence rather than a systematic second-run advantage, but it is
the kind of coincidence worth writing down in case a third replicate ever contradicts it.

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

The measurement-noise limit is now discharged rather than outstanding: arm B2
quantified it, and it is small enough that the two headline findings clear it by an
order of magnitude. What remains unmeasured is whether a *third* run would agree with
the first two — one replicate establishes a floor, not a distribution.

The proposer finding carries its own limit: it rests on one scoring convention
(escalation-as-nonanswer) and the parse rate. The other two conventions move in
the same direction without reaching significance, which is suggestive and nothing
more.
