# Milestone 4: isolated research Witness and advisory Loop Detector

The existing Harborlight Witness is an external service that tails and hash-chains
the decision-log JSONL feed. This milestone adds a separate research evaluator;
it does not change `decision_log.rs`, `DecisionLogEntry`, serve polling, or that
external feed. No CLI or serve integration, production writes or dependencies
are added.

## Research Witness

`FreshWitnessProvider` is a separate trusted factory from `FreshContextProvider`.
Configure it explicitly with `DreamController::with_witness`. Lucid's existing
`witness_calls_per_case` budget opts in to calls; it defaults to zero. Merely
configuring a provider makes no calls. Dream, hypothesis generation and repair
generation never call it.

For each case/repeat, after the final generation sample, the controller builds
`ApprovedEvaluationInput`: only the oracle-free generation input, final answer,
and optional evaluator-owned expected response. Its fields and constructor are
private and it cannot be deserialized. The controller serializes exactly that
projection for the fresh Witness engine. No history, interventions, traces,
memory IDs/content, reader, writer, or permit is passed. Compile-fail doctests
prove that caller JSON cannot fabricate the approved type and that it exposes no
memory-reader method. Approval here means the controller's bounded, privacy-screened
evaluation projection; it is not an operator authentication or policy grant.

The caller-approved base system/soul identity is preserved in the Witness system
prompt, including `base_system_prompt`. It therefore shares the generator's
approved identity text and is not a fully independent persona. Each call uses a distinct fresh context with checked provider identity
and settings, output limits, metering, cancellation and deadlines. Factories must
actually discard prior state and enforce settings; types cannot prove remote
provider isolation or prevent a malicious host adapter from reading external data.

`WitnessAssessment` admits only a test outcome and a hypothetical/simulated
`GeneratedClaim` rationale. Unknown fields, verified trust, NotRun outcomes and
private decoded text are rejected. `LucidReport::witness_evaluations` retains
independent records linked by run, test, arm, repeat, sample and context. They
are separate from final answers and `TestResult`. Call records identify their
Generator/Witness role and actual provider provenance.

`agreement` compares the independent opinion with exact-oracle scoring of the
model's final answer; it is absent when either result is inconclusive. Agreement
is an observation about two assessments, never proof, a trust upgrade, or
authorization. Witness opinions are never supplied to generation or used to
select a repair, override exact scores, or promote memory. `MemoryActor::Witness`
remains rejected by operator authorization tests.

Preflight charges all generator and Witness calls across arms, cases and repeats,
plus hypothesis/repair reservations. Per-repeat Witness calls are bounded to ten
and each evaluates the final sample independently, not best-of-N. Runtime total
budgets combine inference and Witness usage. Failed, timed-out, cancelled and
rejected-output invocations stay counted; rejected sessions before inference do
not count. A Witness failure fails the research run, preserves completed exact
results and prevents returning a repair. No raw provider diagnostics are retained.

## Advisory Loop Detector

`loop_detector::suggest_lucid(&[LoopTurn])` is deterministic and has no I/O, model
calls or execution capability. The caller supplies an ordered conversation's
outputs and source traces. It returns `Option<SuggestedLucid>` and never calls
Lucid. Inputs shorter than three turns yield no suggestion. Longer sequences are
analyzed over their trailing 128 turns while retaining original source ordinals.
Outputs larger than 16KiB are excluded from comparison without compressing turn
order or discarding other outputs.

Normalization lowercases alphanumeric tokens and treats punctuation as separators.
Documented heuristics are:

- The same nonblank normalized output appears at least three times.
- Three consecutive turns each have at least four distinct tokens and adjacent
  token-set Jaccard similarity is at least 0.85 for both pairs.
- At least three within a sliding window of four turns contain hedging tokens (`maybe`, `perhaps`, `might`,
  `uncertain`) or recognized refusal phrases.
- Four consecutive turns repeat their first three non-stopword tokens and each
  has Jaccard similarity of at least 0.6 to the first: possible reasoning fixation.

These are review signals, not diagnoses of defects or intent. A single refusal
cannot produce a suggestion. Repeated refusals produce only advisory safety
review language: preserve legitimate refusals and never bypass them. Every
suggestion is hypothetical and requires operator review. No automatic Lucid
invocation or memory operation exists.

Supporting trace entries contain one-based source-turn ordinals and fingerprints
of their original trace records, not copied conversation/private text. Hedge and
refusal references also record the matching window's original turn range. The caller
resolves these references against its sequence. Existing FNV fingerprints are
correlation aids, not cryptographic integrity or privacy guarantees.

## Phase 1 wrap-up and limits

M1 supplies research contracts with explicit provenance, speculative trust and
budget validation. M2 supplies independent Dream branches and isolated Lucid
reproduction/baseline/intervention experiments with oracle separation and bounded
execution. M3 supplies transactional memory lifecycle/history, immutable sources,
authenticated operator permits and read-only validated retrieval. M4 supplies
opt-in independent Witness opinions and pure advisory loop suggestions.

Offline scripted tests establish these implementation boundaries, not live-model
benefit, causal attribution, trustworthy provider behavior or semantic loop
detection. Witness agreement does not establish truth; loop thresholds can flag
legitimate repetition and miss paraphrased fixation/refusals. Source fingerprints
match exact trimmed input rather than semantic equivalents. Memory's minimal
passing-result evidence gate still requires scoped operator judgment; retrieval
snapshots cannot be revoked mid-flight. Deployment journal mode, host verifier
integration, live adapters and CLI/serve wiring remain future work requiring
separate decisions. Phase 1 grants no production memory or publishing authority.
