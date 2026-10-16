# Phase 2a Lucid pilot preregistration — draft for review

No live outputs have been inspected for these cases. This draft must be approved
and committed before generation. A subsequent explicit run approval is required.
The machine/model choice is supplied by Chris/Claude: i7-4790 CPU-only,
`llama3.2:3b`, local Ollama. Actual digest and provider version will be recorded at
run admission; a model/version change aborts the run rather than pooling results.

## Hypothesis and primary decision

Primary hypothesis: the preregistered `decompose` intervention improves final
exact-match accuracy on the ten candidate challenge tasks relative to the
content-free placebo under equal settings and call budgets. `verify` is a
secondary exploratory contrast, never substituted as primary after seeing results.
Lucid helping requires improvement against placebo, not merely baseline.

Primary metric: pooled correct final answers for decompose minus placebo divided
by 30 target observations (10 tasks x 3 repeats). Every arm may show working and must end with the same output contract:
`You may show working. The LAST line must be exactly: FINAL: <integer>`.
Score only the last non-empty line, strictly matching `^FINAL: (-?\d+)$` using
ASCII digits. Compare the parsed signed 64-bit integer to the oracle integer.
No trimming of non-empty lines or alternate extraction is allowed; empty trailing
lines are ignored and CRLF is treated as a line ending. Leading zeros and -0
compare numerically; overflow, missing/malformed FINAL lines and truncations are
incorrect. Earlier working is allowed. This extraction is preregistered before
outputs, not post-hoc. No oracle changes or best-of-N scoring.
The task, not each stochastic sample, is the conceptual replication unit. Report
per-task contrasts alongside pooled accuracy; the small pilot is descriptive and
cannot establish statistical significance or generalization.

Declare preliminary support only if ALL hold: decompose gains >=3 correct answers
over placebo (>=10 percentage points); it has a positive per-task contrast on
at least 3 of 10 targets; its target accuracy is no worse than clean baseline;
and it loses no more than 1 of 12 correct regression answers versus placebo.
Otherwise the primary pilot shows no demonstrated benefit. A zero/negative
placebo contrast, failure of the regression guard, or universal ceiling performance
is a valid null/no-benefit outcome, not grounds for changing tasks. An aborted or
incomplete run is inconclusive, not success; show partial counts but do not fill
missing observations or extrapolate the complete-run thresholds.

Secondary reporting: verify/placebo and both named/baseline contrasts, all five
arm accuracies, reproduction failure counts, per-task repeat outcomes, latency,
tokens/call counts, refusals and truncations. Do not claim a secondary positive
result rescued a failed primary hypothesis. Multiple-supported interventions may
cause the controller to return no repair; that is independent of the arm metric.

## Arms, settings and sample sizes

Arms in controller order: failure reproduction, clean baseline, decompose, verify,
placebo. Reproduction gets only a candidate-fixture note that failure is not yet
established; there is no prior live conversation. Baseline has no intervention.
Intervention text is frozen in `fixtures/dream_pilot_tasks.json`:

- decompose: identify quantities or symbols, break the task into small steps,
  calculate carefully, end with the shared FINAL-line contract.
- verify: calculate, independently check arithmetic or symbol positions, and
  end with the shared FINAL-line contract.
- placebo: `Answer the question.`

Temperature 0.7, max_output_tokens 256, base seed 20261013, distinct deterministic
per-call seeds recorded in provenance; one sample per case/repeat; repeats 3.
No Witness, memory arm or generated hypotheses. Preserve the approved safety
identity. Request `think:false` to disable Ollama's hidden thinking channel;
visible working in the answer is allowed and scored only through the FINAL line.
Distinct seeds prevent intentional identical reseeding but do not guarantee
independence, reproducibility or actual provider compliance.

Ten target experiments and four standalone regression experiments, five arms
each, three repeats: target n=30/arm; regression n=12/arm. Measurement calls
= (10+4) x 5 x 3 = 210. Reserve one possible repair-description call per experiment
=14, maximum 224 calls. Repairs are never applied and their prose is not an outcome.
No preliminary screening or replacement. The reproduction arm is a fresh live
measurement, not reused screening output. Candidate tasks may all succeed; that
ceiling/null result is retained. All observed failures/refusals remain reported.

## Frozen cases and oracles

Every prompt permits working and requires the identical strict FINAL-line contract.
All arms receive 256 tokens so decomposition can be expressed. Baseline has no
strategy instruction; placebo remains only `Answer the question.`. The runner
uses a preregistered opt-in FinalIntegerScorer; the default exact-trimmed scoring
path stays unchanged. See the design for the scorer and truncation metadata hook.
Report missing/malformed FINAL lines and truncations separately for every arm.
The JSON fixture supplies complete exact prompts, IDs and oracle strings.

| Target | Question | Oracle |
|---|---|---|
| target-01 | 17 multiplied by 19 | 323 |
| target-02 | 27 multiplied by 14 | 378 |
| target-03 | 48 multiplied by 25, then subtract 37 | 1163 |
| target-04 | 37 plus 48 plus 56 minus 29 | 112 |
| target-05 | Six boxes of seven marbles, then give away thirteen | 29 |
| target-06 | Three quarters of 52 | 39 |
| target-07 | Remainder when 97 is divided by 8 | 1 |
| target-08 | Count lowercase i in mississippi | 4 |
| target-09 | Count lowercase r in strawberry | 3 |
| target-10 | Count lowercase t in committee | 2 |
The four regression cases are expected-pass guard cases and may hit a ceiling;
that is valid and does not justify replacement.

| Regression | Question | Oracle |
| regression-01 | 2 plus 2 | 4 |
| regression-02 | 9 multiplied by 3 | 27 |
| regression-03 | Count lowercase a in cat | 1 |
| regression-04 | 12 minus 5 | 7 |

All oracles will be checked offline from arithmetic/string operations before
implementation review; neither screening nor answering these cases via a model
is authorized in this design step. Operator/source notes must distinguish these
suspected challenge cases from previously observed failures.

## Stop rules and time estimate

Sequential only; no concurrency or auto-retry. Stop on first provider/transport
failure, timeout, cancellation, metadata drift, budget violation, or incomplete
experiment. Persist partial JSON/Markdown; report why the run stopped. Do not
restart or tune prompts against these outcomes without a new registration.
Controller limits: call timeout 60s, experiment timeout 600s, global elapsed
150 minutes including cooldowns between experiments, hard cap 224 calls.
Discover `/sys/class/hwmon/hwmon*/` entries whose `name` equals `coretemp` and
read `temp1_input` in millidegrees before EVERY generation call, including repair
calls. Record readings with the call. Stop at >=95C; at >=90C pause and recheck
every 15 seconds until below 90C (global deadline continues). Sensor absence or
unreadability stops before any further generation. The sensor files are the
runner's only filesystem reads outside its task inputs and output artifacts.
Operator cancellation always remains available; no fan, power or hardware settings
are changed. Thermal values are operating rules supplied by Chris/Claude, not
medical/safety guarantees about this machine.

Planning assumption, NOT measured throughput: 15–35 seconds per call with a 256-token ceiling gives about
56–131 minutes for 224 calls, plus at least ~10 minutes of cooldown and metadata/
file overhead: approximately 70–150 minutes, longer if thermal pauses are frequent.
This replaces the original 64-token estimate; no throughput has been measured. Slower inference may hit the stop limits;
an incomplete run remains inconclusive. No timing calibration generation occurs
before preregistration. Local model load/metadata checks do not count as answers.

Artifacts include frozen file fingerprints/git revision, approved identity,
provider/model versions, all call seeds/provenance, exact answers and outcomes,
partial failures, arm metrics and the fixed primary/guard decision. Fixed arm and
task order can confound temperature/elapsed-time effects; the pilot is not a
randomized causal trial. A null result is publishable as a pilot finding after
separate authorization; this task grants no publication or push permission.
