# Dream Engine: research contracts and isolated controller

Shared research contracts live in `sbh_core::dream`, re-exported as
`split_brain_harness::dream`. Milestone 1 adds data and structural validation. Milestone 2 adds the isolated
`split_brain_harness::dream_engine` library controller described below. Neither
connects Dream to the CLI, harness, serve path, or production stores. Milestone 3
adds the explicit operator-side SQLite store and validated reader described in
[DREAM_MEMORY.md](DREAM_MEMORY.md).

## Reuse and architecture inspection

- `sbh-core` owns I/O-free serde contracts. Dream uses its `TraceEntry` and
  `CapabilityRequest` rather than inventing another trace or tool interface.
  Existing records use string identifiers; new transparent ID types prevent
  accidentally mixing task, run, test, context, candidate, and memory identifiers.
- `sbh-llm::InferenceEngine` already provides a stateless prompt/payload boundary;
  `Engines` routes roles. The next controller should use this boundary and a
  scripted implementation, not duplicate backend clients.
- `src/metered.rs` already meters inference calls across roles. Dream contracts
  separate inference and Witness usage while including both in total limits.
  Evaluation preflight uses checked arithmetic and requires multiple repeats.
- `sbh-store` supplies append-only audit, calibration, session, and introspection
  records. `sbh-forge::CapabilityMemory` supplies JSON capability-pattern memory
  and reputation. Neither is a transactional Dream candidate/revision store.
  At the M1 inspection there was no SQLite implementation. M3 adds a separate
  transactional research memory store using the approved bundled rusqlite dependency.
- `sbh-safety` already owns soul loading, capability policy, and redaction.
  The forge can execute generated tools; Dream never invokes it.
- The harness refinement/arbitrator pipeline already records trace and model
  provenance, but is telemetry analysis, not an independent Dream/Lucid experiment
  runner. The transformer owns soul/RAG prompt assembly; it is unchanged.
- R-LLM's architecture and memory schemas keep hemispheres stateless and memory
  operations in an external owner. Its drift/Witness alerts inform later advisory
  monitoring. Python prototypes and Dreaming remain read-only references.

## Contracts and admission

All generated branch, hypothesis, repair, intervention, and advisory prose uses
`GeneratedClaim`, whose required trust field permits only hypothetical or
simulated information. `EvidenceClaim` distinguishes all five trust categories;
observed evidence needs a run ID, external/operator verification also needs a
reference. These are assertions to be checked, not proofs of truth or authority.
`EvidenceClaim`, `RepairCandidate`, and `TrustClassification` must never be
deserialized from model output. They are constructed by the controller from
independent evaluation; all model prose is admitted as `GeneratedClaim` only.
Repair admission requires at least one observed or verified supporting claim.
`SuggestedLucid` always requires operator review and a non-empty rationale and
supporting trace; there is no review opt-out field.

Unknown fields are rejected on records. ID construction and deserialization reject
blank IDs. Other semantic constraints require an explicit `validate()` call after
construction/deserialization, matching the existing capability admission pattern.
Do not treat successful JSON parsing as successful admission.

`TestSpec` keeps the optional evaluator oracle. `generation_input()` produces an
oracle-free projection; generation code must use that projection. This does not
screen original prompts, failure conversations, or traces for oracle leakage.
The next controller must screen them and generated hypotheses/interventions/repairs,
including regression answers, before using any of them as prompts.

Source provenance and later evidence run IDs have separate fields. Source
provenance must be preserved on revisions by the future store; this milestone
has no persistence or immutable-history enforcement. Provider/model versions may
be unknown explicitly. Fingerprints are correlations, not integrity proofs.

Arm records require different context IDs and equal declared evaluation budgets.
IDs do not establish that a provider reset context. Final-answer outcomes and
best-of-N outcomes are separate. Test failures and declared regressions remain
visible even if one branch succeeded. Repeated results do not prove causality.

Memory revision requests declare evidence, run, revision, and operator approval
references. A reference string is not authenticated authorization. M3's operator
store verifies scoped approval, run admission, state transitions and revision
concurrency in a transaction. Rollback restores only Candidate; re-promotion
requires fresh approval. Core contracts themselves grant no write authority.

## Milestone 1 limits and handoff

Actual guarantees here: no I/O or execution in the new module, speculative-only
generated trust, oracle-free input projection, structural evidence/budget checks,
and no operation that applies a repair or promotes memory. No soul, safety policy,
release metadata, production memory, or existing runtime behavior is changed.

Runtime isolation, cancellation, same-task memory exclusion, Witness evaluation
input approval and isolation, safety-refusal interpretation, authenticated
promotion, transactional revision history, and immutable provenance are deferred.
Milestone 1 tests cover records and validation, not runtime security properties.

Recommended next step: Milestone 2, an isolated controller over the existing
inference trait with deterministic scripted-provider tests. Each Dream branch and
Lucid arm needs a fresh context; memory recombination must be opt-in and exclude
same-task memory. Preflight budgets must include hypothesis and repair/synthesis
calls as well as evaluation and Witness calls. The current Lucid preflight reserves
one generation call per permitted hypothesis and comparable evaluation arms;
any later repair/synthesis generation needs an additional explicit reservation.
This is the preserved Milestone 1 scope; Milestone 2 implementation follows below.


## Milestone 2: isolated controller

`DreamController::new(provider, config)` accepts a caller-authorized
`FreshContextProvider` and an already approved base system/soul prompt. It never
loads credentials, initializes a live backend, or writes files. M3 adds opt-in
retrieval through a configured `ValidatedMemoryReader`.
`dream(&DreamRequest, &Cancellation)` and
`lucid(&LucidExperiment, &Cancellation)` are callable async library tools.
No harness/transformer/serve behavior is changed. M2 added no dependencies; M3
adds rusqlite only to sbh-store.

### Provider and context boundary

The factory creates a new `EngineSession` over the existing `InferenceEngine`
for every inference call, including each repeated case/sample. Engines are used
once, metered with existing `MeteredEngine`, and dropped after the call. Fresh
context IDs are unique within a process even if a caller reuses its run ID.
The factory receives settings; the controller rejects sessions reporting different
settings or provider/model identity. Each call records versions, settings, context,
and the fingerprint of the actual augmented system prompt. Unknown versions stay
explicitly unknown.

The factory is a trusted adapter boundary. It must be nonblocking, actually create
fresh context, truthfully report identity/settings, enforce the declared seed and
output-token ceiling, and drop/cancel its own pending work. The generic inference
trait cannot verify a remote provider's internal state, tokenizer, or truthful
reporting. There is deliberately no adapter wrapping a shared engine or silently
claiming unsupported settings. This milestone tests a scripted offline factory;
no live backend adapter is supplied or empirically validated.

### Dream and memory experiment arms

The interpretive and remote-association branches receive the original prompt
independently. Synthesis receives only their explicitly serialized speculative
claims in another fresh context. Digital Hallucination is labeled speculative
research in every stage; generated claims can only be hypothetical or simulated.

Normal branches receive no memory. `memory_recombination: Some(ids)` adds a
separate three-call branch/synthesis arm when the controller has a validated store
reader. The reader selects current Validated heads, excluding Candidate, Retired,
absent, same-task/run and identical trimmed source-input records, including prompts
submitted under renamed task IDs. Excluded IDs are reported. Empty, duplicate,
excessive or entirely ineligible selections are rejected before inference. Caller
content cannot assert validation state. The controller has no memory write methods.

Dream reports keep `ExperimentRun` accounting, independent `BranchProposal`
records, and the explicitly optional memory arm. Dream's branch proposals are not
Lucid evaluation arms; its `ExperimentRun.arms` remains empty. All calls, including
synthesis, have their own `CallRecord` provenance.

### Lucid execution and evidence

Each admitted run evaluates FailureReproduction and CleanBaseline first, then all
named and permitted generated interventions independently. Original failing
conversation/trace are included only in reproduction of the original case;
regressions and the baseline have clean input. Every arm evaluates the same
original/regression cases with identical repeat/sample counts, settings, token
ceilings, and per-call timeout. Each sample has a fresh engine and receives no
output from preceding samples or arms.

Hypothesis generation is explicit opt-in and bounded to ten. Model output is
parsed only as `GeneratedClaim`; the controller constructs hypotheses, evidence,
and repairs. Evaluator oracles never enter serialized generation inputs. Original
and regression prompts/identifiers, failing conversations, all trace text fields,
base prompts, named interventions, and generated hypothesis/intervention/repair
text are screened against all known answers. Short oracles use word boundaries;
longer ones use conservative case-insensitive substring matching. Screening catches
literal leakage and decoded JSON escapes, not paraphrases, encodings, or all
semantic leakage. Benign text can still be rejected.

An evaluator uses exact, case-sensitive, trimmed final-answer matching. With no
oracle its outcome is Inconclusive. The last sample supplies the scored final
answer; best-of-N is a separate diagnostic and cannot create an apparent repair.
There is no model judge or Witness call.

A proposed repair requires that every repeated reproduction failed and exactly
one intervention passed every repeated original-case outcome. Zero or multiple
supported interventions produce no repair; all supported interventions are
recorded in the trace, and multiple support is explicitly inconclusive. The
unique qualifying intervention is described in a single generated proposal. Its outcomes, observed
support, counterevidence, uncertainty, and source provenance remain structured.
A regression is declared only when the clean baseline passed all repeats and the
intervention failed at least one. Failed or inconclusive regression evaluations
remain counterevidence even when a new regression is not established. If the clean
baseline already passes the original case, unique context attribution is explicitly
inconclusive. Repeated samples support a proposal, not a proof of causality.
No method applies the repair. No reproduced failure or no qualifying intervention
returns a completed research report with no repair; it is not a successful fix.

### Budgets, errors and cancellation

Admission reserves all worst-case calls before any model invocation:

- Dream: three normal branch/synthesis calls, plus three for an optional memory arm.
- Lucid: `(2 + named_interventions + permitted_hypotheses) * cases * repeats *
  inference_calls_per_case + permitted_hypotheses + 1 repair_call`.

Limits include 1..10,000 total calls, 2..10 repeats, 1..10 samples per case,
32 total test cases, 32 named interventions, 128 trace entries, temperature 0..2,
and 1..8,192 output tokens. Combined outbound prompt bytes, returned output bytes,
per-call timeouts (at most 120 seconds), and total run time (at most 600 seconds)
are independently bounded. Caller-supplied limits can be smaller. Backend token
settings remain a factory responsibility; byte limits enforce response admission,
not the allocation behavior of a malicious provider.

Witness budgets are rejected because Witness execution belongs to Milestone 4;
its usage stays zero rather than pretending reserved Witness calls happened.
Both normal and generation calls charge the run total. Failed, cancelled, and
timed-out inference invocations remain counted. Partial successful observations
and call metadata are retained on failure; raw private/provider diagnostic text is
not. Preflight rejection returns an error with zero model calls. Runtime rejection
returns a Failed report. Every output run passes `ExperimentRun.validate()`
before completion; invalid structural records become RejectedOutput failures. `CallRecord.succeeded` means the provider response passed
transport/privacy admission, not that a generated proposal was accepted or correct.

`Cancellation` is sticky and wakes a pending call. Cancellation/timeout drops the
pending future and owned engine; the provider must honor disposal for remote work.
Cancellation between calls stops subsequent work. A completed final reply wins a
simultaneous cancellation signal, and all completions remain in-memory.
Existing `security::redact` supplies conservative privacy admission; its detection
is incomplete and may reject benign text. No new security policy is introduced.

### Verification and remaining scope

`tests/dream_controller.rs` uses the existing inference trait with scripted
providers and asserts independent payloads, no previous-run state, synthesis-only
branch sharing, provenance/settings, memory exclusion, baseline/intervention
comparability, original and regression oracle screening, regression reporting,
final-answer scoring, bounded call reservations, cancellation, timeout, provider
failure, privacy admission, and disposal. Tests do not establish live-model benefit.

Milestone 3 persistence and authenticated operator validation are described in
[DREAM_MEMORY.md](DREAM_MEMORY.md). Witness/input approval and the advisory loop
detector remain Milestone 4. Neither is implemented here.
