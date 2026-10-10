# Dream Engine: Milestone 1

Shared research contracts live in `sbh_core::dream`, re-exported as
`split_brain_harness::dream`. This milestone adds data and structural validation
only. It does not connect Dream to the CLI, harness, models, or production stores.

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
  No SQLite dependency or implementation exists in this workspace; transactional
  persistence must be designed in Milestone 3, not assumed present.
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
references. A reference string is not authenticated authorization. No method
promotes memory. A future transactional store must verify authorized operators,
run existence/admission, state transitions, and revision concurrency. Rollback
requests can restore only Candidate; re-promotion must require fresh approval.

## Limits and next milestone

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
No subsequent milestone is implemented here.
