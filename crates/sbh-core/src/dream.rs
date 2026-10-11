//! I/O-free Dream Engine research contracts (Milestone 1).
//!
//! These records grant no execution, memory, approval, or network authority.
//! Call `validate` at admission boundaries; serde alone validates shape, not
//! evidence authenticity. Evaluator-only oracles must never enter generation.
//! `EvidenceClaim`, `RepairCandidate`, and `TrustClassification` are controller-
//! owned records: NEVER deserialize them from model output. Model prose is
//! admitted only as `GeneratedClaim`; the controller constructs evidence records.

use serde::{Deserialize, Serialize};

use crate::{capability::CapabilityRequest, types::TraceEntry};

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                if value.trim().is_empty() {
                    return Err(concat!(stringify!($name), " must not be blank").into());
                }
                Ok(Self(value))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
identifier!(RunId);
identifier!(TaskId);
identifier!(CandidateId);
identifier!(TestId);
identifier!(ContextId);
identifier!(MemoryId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamMode {
    Dream,
    DigitalHallucination,
    Lucid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustClassification {
    Hypothetical,
    Simulated,
    Observed,
    ExternallyVerified,
    OperatorValidated,
}

/// Generated prose cannot claim observed, external, or operator verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeculativeTrust {
    Hypothetical,
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedClaim {
    pub text: String,
    pub trust: SpeculativeTrust,
}
impl GeneratedClaim {
    pub fn validate(&self) -> Result<(), String> {
        nonblank(&self.text, "generated claim")
    }
}

/// An evidence assertion, never a credential or proof of authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceClaim {
    pub statement: String,
    pub trust: TrustClassification,
    pub evidence: String,
    pub evidence_run_id: Option<RunId>,
    /// Reference to external verification or operator audit, not a boolean grant.
    pub verification_reference: Option<String>,
}
impl EvidenceClaim {
    pub fn validate(&self) -> Result<(), String> {
        nonblank(&self.statement, "evidence statement")?;
        nonblank(&self.evidence, "evidence")?;
        if matches!(
            self.trust,
            TrustClassification::Observed
                | TrustClassification::ExternallyVerified
                | TrustClassification::OperatorValidated
        ) && self.evidence_run_id.is_none()
        {
            return Err("observed or verified evidence requires an evidence run".into());
        }
        if matches!(
            self.trust,
            TrustClassification::ExternallyVerified | TrustClassification::OperatorValidated
        ) {
            nonblank(
                self.verification_reference.as_deref().unwrap_or(""),
                "verification reference",
            )?;
        }
        Ok(())
    }
}

/// Source identity is separate from later evidence runs. No credentials or
/// backend endpoint are recorded. Unknown versions stay explicit (`None`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceRecord {
    pub source_run_id: RunId,
    pub source_task_id: TaskId,
    pub provider: String,
    pub provider_version: Option<String>,
    pub model: String,
    pub model_version: Option<String>,
    pub temperature: f32,
    pub seed: Option<u64>,
    pub max_output_tokens: u32,
    pub system_prompt_fingerprint: String,
}
impl ProvenanceRecord {
    pub fn validate(&self) -> Result<(), String> {
        nonblank(&self.provider, "provider")?;
        nonblank(&self.model, "model")?;
        nonblank(&self.system_prompt_fingerprint, "system prompt fingerprint")?;
        if !self.temperature.is_finite() || self.temperature < 0.0 || self.max_output_tokens == 0 {
            return Err("invalid inference settings".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchKind {
    Interpretive,
    RemoteAssociation,
    Synthesis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchProposal {
    pub id: CandidateId,
    pub branch: BranchKind,
    pub context_id: ContextId,
    pub claims: Vec<GeneratedClaim>,
    pub source: ProvenanceRecord,
    /// Proposal only; existing capability policy must authorize any later use.
    pub proposed_capability: Option<CapabilityRequest>,
}
impl BranchProposal {
    pub fn validate(&self) -> Result<(), String> {
        self.source.validate()?;
        if self.claims.is_empty() {
            return Err("branch requires a claim".into());
        }
        for claim in &self.claims {
            claim.validate()?;
        }
        if let Some(request) = &self.proposed_capability {
            request.validate()?;
        }
        Ok(())
    }
}

/// Only this oracle-free projection is eligible for hypothesis/repair prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationInput {
    pub task_id: TaskId,
    pub original_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestSpec {
    pub id: TestId,
    pub task_id: TaskId,
    pub original_prompt: String,
    /// Evaluator-only. Never include the serialized TestSpec in model prompts.
    pub expected_response: Option<String>,
}
impl TestSpec {
    pub fn generation_input(&self) -> GenerationInput {
        GenerationInput {
            task_id: self.task_id.clone(),
            original_prompt: self.original_prompt.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedIntervention {
    pub name: String,
    pub instruction: GeneratedClaim,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HypothesisCandidate {
    pub id: CandidateId,
    pub claim: GeneratedClaim,
    pub intervention: NamedIntervention,
    pub source: ProvenanceRecord,
}

impl HypothesisCandidate {
    pub fn validate(&self) -> Result<(), String> {
        self.claim.validate()?;
        nonblank(&self.intervention.name, "intervention name")?;
        self.intervention.instruction.validate()?;
        self.source.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    Passed,
    Failed,
    Inconclusive,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestResult {
    pub test_id: TestId,
    pub evidence_run_id: RunId,
    pub outcome: TestOutcome,
    pub final_answer: String,
    /// Separate diagnostic; never used as the final-answer score.
    pub best_of_n_outcome: Option<TestOutcome>,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairCandidate {
    pub id: CandidateId,
    pub proposal: GeneratedClaim,
    pub source: ProvenanceRecord,
    pub evidence_run_id: RunId,
    pub intervention: NamedIntervention,
    pub evidence_for: Vec<EvidenceClaim>,
    pub evidence_against: Vec<EvidenceClaim>,
    pub uncertainty: String,
    pub test_results: Vec<TestResult>,
    pub observed_regressions: Vec<TestId>,
}
impl RepairCandidate {
    pub fn validate(&self) -> Result<(), String> {
        self.proposal.validate()?;
        self.source.validate()?;
        self.intervention.instruction.validate()?;
        nonblank(&self.intervention.name, "intervention name")?;
        nonblank(&self.uncertainty, "uncertainty")?;
        if !self.evidence_for.iter().any(|claim| {
            matches!(
                claim.trust,
                TrustClassification::Observed
                    | TrustClassification::ExternallyVerified
                    | TrustClassification::OperatorValidated
            )
        }) || self.test_results.is_empty()
        {
            return Err(
                "repair requires observed or verified supporting evidence and test outcomes".into(),
            );
        }
        for claim in self.evidence_for.iter().chain(&self.evidence_against) {
            claim.validate()?;
            if claim.evidence_run_id.as_ref() != Some(&self.evidence_run_id) {
                return Err("repair evidence must reference its evidence run".into());
            }
        }
        for result in &self.test_results {
            nonblank(&result.evidence, "test evidence")?;
            if result.evidence_run_id != self.evidence_run_id {
                return Err("test outcome belongs to a different evidence run".into());
            }
        }
        for id in &self.observed_regressions {
            if !self
                .test_results
                .iter()
                .any(|r| &r.test_id == id && r.outcome == TestOutcome::Failed)
            {
                return Err("observed regression requires a failed test outcome".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Candidate,
    Validated,
    Retired,
}

/// Declaration for a future authorized, transactional store operation. A
/// deserialized request is NOT operator authorization. Rollback requires fresh
/// validation before re-promotion; it can only restore a Candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryRevisionRequest {
    /// Edit untrusted content only while Candidate; authorization and revision
    /// checks belong to the transactional operator-side store.
    ReviseCandidate {
        memory_id: MemoryId,
        expected_revision: u64,
        content: GeneratedClaim,
        reason: String,
    },
    Promote {
        memory_id: MemoryId,
        expected_revision: u64,
        operator_approval_reference: String,
        evidence: String,
        evidence_run_id: RunId,
    },
    Retire {
        memory_id: MemoryId,
        expected_revision: u64,
        reason: String,
    },
    RollbackToCandidate {
        memory_id: MemoryId,
        expected_revision: u64,
        restore_revision: u64,
        reason: String,
    },
}
impl MemoryRevisionRequest {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::ReviseCandidate {
                content, reason, ..
            } => {
                content.validate()?;
                nonblank(reason, "candidate revision reason")
            }
            Self::Promote {
                operator_approval_reference,
                evidence,
                ..
            } => {
                nonblank(operator_approval_reference, "operator approval reference")?;
                nonblank(evidence, "promotion evidence")
            }
            Self::Retire { reason, .. } => nonblank(reason, "retirement reason"),
            Self::RollbackToCandidate {
                expected_revision,
                restore_revision,
                reason,
                ..
            } => {
                nonblank(reason, "rollback reason")?;
                if restore_revision >= expected_revision {
                    return Err("rollback requires an earlier revision".into());
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentArm {
    FailureReproduction,
    CleanBaseline,
    Intervention {
        name: String,
    },
    /// Explicit opt-in arm; the controller must exclude same-task source memory.
    MemoryRecombination {
        memory_ids: Vec<MemoryId>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationBudget {
    pub inference_calls_per_case: u32,
    pub witness_calls_per_case: u32,
    pub repeats: u32,
    pub max_output_tokens: u32,
}
impl EvaluationBudget {
    /// Preflight all comparable arms and all cases, including Witness calls.
    pub fn required_calls(&self, arms: u32, cases: u32) -> Result<u64, String> {
        if self.repeats < 2
            || self.inference_calls_per_case == 0
            || self.max_output_tokens == 0
            || arms == 0
            || cases == 0
        {
            return Err(
                "evaluation requires nonzero arms/cases/budgets and at least two repeats".into(),
            );
        }
        (u64::from(self.inference_calls_per_case) + u64::from(self.witness_calls_per_case))
            .checked_mul(u64::from(self.repeats))
            .and_then(|n| n.checked_mul(u64::from(arms)))
            .and_then(|n| n.checked_mul(u64::from(cases)))
            .ok_or_else(|| "evaluation call count overflow".into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Planned,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmRecord {
    pub arm: ExperimentArm,
    pub context_id: ContextId,
    pub budget: EvaluationBudget,
    pub provenance: ProvenanceRecord,
    pub results: Vec<TestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRun {
    pub id: RunId,
    pub task_id: TaskId,
    pub mode: DreamMode,
    pub status: RunStatus,
    pub arms: Vec<ArmRecord>,
    pub inference_calls: u64,
    pub witness_calls: u64,
    pub max_total_calls: u64,
    pub trace: Vec<TraceEntry>,
}
impl ExperimentRun {
    /// Structural checks only: context IDs cannot prove provider context reset.
    pub fn validate(&self) -> Result<(), String> {
        if self
            .inference_calls
            .checked_add(self.witness_calls)
            .is_none_or(|n| n > self.max_total_calls)
        {
            return Err("run exceeds total call budget".into());
        }
        let mut contexts = std::collections::HashSet::new();
        let baseline_budget = self.arms.first().map(|arm| arm.budget);
        for arm in &self.arms {
            if !contexts.insert(&arm.context_id) {
                return Err("experiment arms must have distinct contexts".into());
            }
            if Some(arm.budget) != baseline_budget {
                return Err("experiment arms require equivalent evaluation budgets".into());
            }
            arm.budget.required_calls(1, 1)?;
            arm.provenance.validate()?;
            if arm.provenance.source_run_id != self.id
                || arm.provenance.source_task_id != self.task_id
            {
                return Err("arm provenance must identify this run and task".into());
            }
            for result in &arm.results {
                if result.evidence_run_id != self.id {
                    return Err("arm test outcome belongs to a different run".into());
                }
                nonblank(&result.evidence, "test evidence")?;
            }
        }
        Ok(())
    }
}

/// Advisory data only. Refusal by itself is not evidence of a defect; callers
/// must review legitimate safety behavior before requesting research.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestedLucid {
    pub rationale: GeneratedClaim,
    pub supporting_trace: Vec<TraceEntry>,
}

impl SuggestedLucid {
    /// Operator review is always required; there is no opt-out field.
    pub fn validate(&self) -> Result<(), String> {
        self.rationale.validate()?;
        if self.supporting_trace.is_empty() {
            return Err("Lucid suggestion requires supporting trace".into());
        }
        Ok(())
    }
}

fn nonblank(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{label} must not be blank"))
    } else {
        Ok(())
    }
}

/// Evaluator/controller input. Only `failure.generation_input()` is eligible
/// for generation; traces and conversations may contain the oracle and need
/// screening before any future controller includes them in prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LucidRequest {
    pub failure: TestSpec,
    pub failure_trace: Vec<TraceEntry>,
    pub failing_conversation: Option<String>,
    pub interventions: Vec<NamedIntervention>,
    pub max_hypotheses: u32,
    pub regression_cases: Vec<TestSpec>,
    pub allow_untrusted_hypotheses: bool,
    pub evaluation_budget: EvaluationBudget,
    pub max_total_calls: u64,
}
impl LucidRequest {
    pub fn validate(&self) -> Result<(), String> {
        nonblank(&self.failure.original_prompt, "original prompt")?;
        if self.failure_trace.is_empty()
            && self
                .failing_conversation
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
        {
            return Err("Lucid requires a failure trace or failing conversation".into());
        }
        if self.allow_untrusted_hypotheses && self.max_hypotheses == 0 {
            return Err("hypothesis generation requires a positive hypothesis limit".into());
        }
        if self.interventions.is_empty() && !self.allow_untrusted_hypotheses {
            return Err("Lucid requires named interventions or permitted hypotheses".into());
        }
        let mut names = std::collections::HashSet::new();
        for intervention in &self.interventions {
            nonblank(&intervention.name, "intervention name")?;
            intervention.instruction.validate()?;
            if !names.insert(intervention.name.trim()) {
                return Err("intervention names must be unique".into());
            }
        }
        let mut test_ids = std::collections::HashSet::from([&self.failure.id]);
        for case in &self.regression_cases {
            nonblank(&case.original_prompt, "regression prompt")?;
            if !test_ids.insert(&case.id) {
                return Err("test IDs must be unique".into());
            }
        }
        // Reproduction + baseline + named interventions + worst-case generated
        // hypotheses; one generation call per hypothesis is charged separately.
        let hypotheses = if self.allow_untrusted_hypotheses {
            self.max_hypotheses
        } else {
            0
        };
        let arms = u32::try_from(self.interventions.len())
            .ok()
            .and_then(|n| n.checked_add(2))
            .and_then(|n| n.checked_add(hypotheses))
            .ok_or("too many experiment arms")?;
        let cases = u32::try_from(self.regression_cases.len())
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or("too many test cases")?;
        let required = self
            .evaluation_budget
            .required_calls(arms, cases)?
            .checked_add(u64::from(hypotheses))
            .ok_or("call count overflow")?;
        if required > self.max_total_calls {
            return Err("insufficient call budget for planned Lucid experiment".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn run_id(value: &str) -> RunId {
        value.to_string().try_into().unwrap()
    }
    fn task_id() -> TaskId {
        "task-1".to_string().try_into().unwrap()
    }
    fn test_id(value: &str) -> TestId {
        value.to_string().try_into().unwrap()
    }
    fn claim() -> GeneratedClaim {
        GeneratedClaim {
            text: "Try an independent clean context".into(),
            trust: SpeculativeTrust::Hypothetical,
        }
    }
    fn provenance() -> ProvenanceRecord {
        ProvenanceRecord {
            source_run_id: run_id("source-run"),
            source_task_id: task_id(),
            provider: "scripted".into(),
            provider_version: Some("1".into()),
            model: "offline-fixture".into(),
            model_version: None,
            temperature: 0.1,
            seed: Some(7),
            max_output_tokens: 64,
            system_prompt_fingerprint: "fixture-fingerprint".into(),
        }
    }
    fn evidence() -> EvidenceClaim {
        EvidenceClaim {
            statement: "Two repeat trials passed".into(),
            trust: TrustClassification::Observed,
            evidence: "fixture outputs matched the evaluator oracle".into(),
            evidence_run_id: Some(run_id("evidence-run")),
            verification_reference: None,
        }
    }
    fn result(id: &str, outcome: TestOutcome) -> TestResult {
        TestResult {
            test_id: test_id(id),
            evidence_run_id: run_id("evidence-run"),
            outcome,
            final_answer: "fixture output".into(),
            best_of_n_outcome: Some(TestOutcome::Passed),
            evidence: "scripted test evaluation".into(),
        }
    }
    fn repair() -> RepairCandidate {
        RepairCandidate {
            id: "repair-1".to_string().try_into().unwrap(),
            proposal: claim(),
            source: provenance(),
            evidence_run_id: run_id("evidence-run"),
            intervention: NamedIntervention {
                name: "clean-context".into(),
                instruction: claim(),
            },
            evidence_for: vec![evidence()],
            evidence_against: vec![],
            uncertainty: "Offline fixtures establish no live-model causality".into(),
            test_results: vec![
                result("original", TestOutcome::Passed),
                result("regression", TestOutcome::Failed),
            ],
            observed_regressions: vec![test_id("regression")],
        }
    }
    fn budget() -> EvaluationBudget {
        EvaluationBudget {
            inference_calls_per_case: 1,
            witness_calls_per_case: 1,
            repeats: 2,
            max_output_tokens: 64,
        }
    }
    fn experiment() -> ExperimentRun {
        let mut source = provenance();
        source.source_run_id = run_id("evidence-run");
        let arm = |kind, context: &str| ArmRecord {
            arm: kind,
            context_id: context.to_string().try_into().unwrap(),
            budget: budget(),
            provenance: source.clone(),
            results: vec![result("original", TestOutcome::Passed)],
        };
        ExperimentRun {
            id: run_id("evidence-run"),
            task_id: task_id(),
            mode: DreamMode::Lucid,
            status: RunStatus::Completed,
            arms: vec![
                arm(ExperimentArm::CleanBaseline, "baseline-context"),
                arm(
                    ExperimentArm::Intervention {
                        name: "clean-context".into(),
                    },
                    "intervention-context",
                ),
            ],
            inference_calls: 4,
            witness_calls: 4,
            max_total_calls: 8,
            trace: vec![],
        }
    }
    fn request() -> LucidRequest {
        LucidRequest {
            failure: TestSpec {
                id: test_id("original"),
                task_id: task_id(),
                original_prompt: "Question".into(),
                expected_response: Some("SECRET_ORACLE".into()),
            },
            failure_trace: vec![],
            failing_conversation: Some("Original failing conversation".into()),
            interventions: vec![NamedIntervention {
                name: "clean-context".into(),
                instruction: claim(),
            }],
            max_hypotheses: 1,
            regression_cases: vec![],
            allow_untrusted_hypotheses: false,
            evaluation_budget: budget(),
            max_total_calls: 12,
        }
    }
    fn roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let encoded = serde_json::to_value(value).unwrap();
        let decoded: T = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }

    #[test]
    fn all_requested_contracts_roundtrip() {
        for mode in [
            DreamMode::Dream,
            DreamMode::DigitalHallucination,
            DreamMode::Lucid,
        ] {
            roundtrip(&mode);
        }
        for state in [
            MemoryState::Candidate,
            MemoryState::Validated,
            MemoryState::Retired,
        ] {
            roundtrip(&state);
        }
        for trust in [
            TrustClassification::Hypothetical,
            TrustClassification::Simulated,
            TrustClassification::Observed,
            TrustClassification::ExternallyVerified,
            TrustClassification::OperatorValidated,
        ] {
            roundtrip(&trust);
        }
        let branch = BranchProposal {
            id: "branch-1".to_string().try_into().unwrap(),
            branch: BranchKind::RemoteAssociation,
            context_id: "fresh-context".to_string().try_into().unwrap(),
            claims: vec![claim()],
            source: provenance(),
            proposed_capability: None,
        };
        branch.validate().unwrap();
        roundtrip(&branch);
        roundtrip(&HypothesisCandidate {
            id: "hypothesis-1".to_string().try_into().unwrap(),
            claim: claim(),
            intervention: request().interventions.remove(0),
            source: provenance(),
        });
        roundtrip(&repair());
        roundtrip(&evidence());
        roundtrip(&request().failure);
        roundtrip(&experiment());
        roundtrip(&provenance());
        roundtrip(&request());
        roundtrip(&SuggestedLucid {
            rationale: claim(),
            supporting_trace: vec![],
        });
    }

    #[test]
    fn generated_claims_require_speculative_trust() {
        assert!(serde_json::from_value::<GeneratedClaim>(json!({"text":"claim"})).is_err());
        for trust in ["observed", "externally_verified", "operator_validated"] {
            assert!(serde_json::from_value::<GeneratedClaim>(
                json!({"text":"claim", "trust":trust})
            )
            .is_err());
        }
        assert!(serde_json::from_value::<GeneratedClaim>(
            json!({"text":"claim", "trust":"simulated", "authorized":true})
        )
        .is_err());
    }

    #[test]
    fn identifiers_reject_blank_json_and_construction() {
        assert!(RunId::try_from("  ".to_string()).is_err());
        assert!(serde_json::from_str::<RunId>("\" \"").is_err());
        assert!(serde_json::from_str::<MemoryId>("\"\"").is_err());
    }

    #[test]
    fn evidence_rejects_missing_support_and_verification() {
        let mut e = evidence();
        e.evidence = " ".into();
        assert!(e.validate().is_err());
        e = evidence();
        e.evidence_run_id = None;
        assert!(e.validate().is_err());
        e = evidence();
        e.trust = TrustClassification::ExternallyVerified;
        assert!(e.validate().is_err());
        e.verification_reference = Some("external-audit-1".into());
        e.validate().unwrap();
    }

    #[test]
    fn oracle_is_excluded_from_generation_projection() {
        let input = request().failure.generation_input();
        let json = serde_json::to_string(&input).unwrap();
        assert!(!json.contains("SECRET_ORACLE"));
        assert!(!json.contains("expected_response"));
        assert!(serde_json::from_value::<GenerationInput>(
            json!({"task_id":"t", "original_prompt":"q", "expected_response":"answer"})
        )
        .is_err());
    }

    #[test]
    fn repair_preserves_source_and_separate_evidence_run() {
        let r = repair();
        r.validate().unwrap();
        let restored: RepairCandidate =
            serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(restored.source.source_run_id, run_id("source-run"));
        assert_eq!(restored.evidence_run_id, run_id("evidence-run"));
    }

    #[test]
    fn repair_rejects_missing_or_mismatched_evidence() {
        let mut r = repair();
        r.evidence_for.clear();
        assert!(r.validate().is_err());
        r = repair();
        r.evidence_for[0].evidence_run_id = Some(run_id("other"));
        assert!(r.validate().is_err());
        r = repair();
        r.test_results[0].evidence_run_id = run_id("other");
        assert!(r.validate().is_err());
        r = repair();
        r.uncertainty.clear();
        assert!(r.validate().is_err());
    }

    #[test]
    fn failed_regression_is_reported_separately_from_best_of_n() {
        let mut r = repair();
        r.validate().unwrap();
        assert_eq!(r.test_results[1].outcome, TestOutcome::Failed);
        assert_eq!(
            r.test_results[1].best_of_n_outcome,
            Some(TestOutcome::Passed)
        );
        r.observed_regressions.push(test_id("nonexistent"));
        assert!(r.validate().is_err());
    }

    #[test]
    fn promotion_request_requires_evidence_and_approval_reference() {
        let value = json!({"operation":"promote", "memory_id":"m", "expected_revision":1,
            "operator_approval_reference":"operator-audit-1", "evidence":"reviewed fixture", "evidence_run_id":"r"});
        let request: MemoryRevisionRequest = serde_json::from_value(value.clone()).unwrap();
        request.validate().unwrap();
        roundtrip(&request);
        for field in ["evidence", "operator_approval_reference"] {
            let mut invalid = value.clone();
            invalid[field] = Value::String(" ".into());
            assert!(serde_json::from_value::<MemoryRevisionRequest>(invalid)
                .unwrap()
                .validate()
                .is_err());
        }
        let mut invalid = value;
        invalid.as_object_mut().unwrap().remove("evidence_run_id");
        assert!(serde_json::from_value::<MemoryRevisionRequest>(invalid).is_err());
    }

    #[test]
    fn rollback_contract_cannot_restore_validated_state() {
        let value = json!({"operation":"rollback_to_candidate", "memory_id":"m", "expected_revision":3, "restore_revision":1, "reason":"regression"});
        let request: MemoryRevisionRequest = serde_json::from_value(value.clone()).unwrap();
        request.validate().unwrap();
        roundtrip(&request);
        let mut invalid = value;
        invalid["target_state"] = json!("validated");
        assert!(serde_json::from_value::<MemoryRevisionRequest>(invalid).is_err());
        assert!(MemoryRevisionRequest::RollbackToCandidate {
            memory_id: "m".to_string().try_into().unwrap(),
            expected_revision: 3,
            restore_revision: 3,
            reason: "same".into()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn comparable_arms_require_distinct_contexts_and_equal_budgets() {
        let mut run = experiment();
        run.validate().unwrap();
        run.arms[1].context_id = run.arms[0].context_id.clone();
        assert!(run.validate().is_err());
        run = experiment();
        run.arms[1].budget.repeats = 3;
        assert!(run.validate().is_err());
        run = experiment();
        run.arms[1].provenance.source_task_id = "other".to_string().try_into().unwrap();
        assert!(run.validate().is_err());
    }

    #[test]
    fn witness_calls_are_separate_but_charge_total_budget() {
        let mut run = experiment();
        run.validate().unwrap();
        assert_eq!(run.inference_calls, 4);
        assert_eq!(run.witness_calls, 4);
        run.max_total_calls = 7;
        assert!(run.validate().is_err());
        run.inference_calls = u64::MAX;
        assert!(run.validate().is_err());
    }

    #[test]
    fn repeated_evaluation_preflight_counts_all_arms_and_cases() {
        assert_eq!(budget().required_calls(3, 2).unwrap(), 24);
        let mut b = budget();
        b.repeats = 1;
        assert!(b.required_calls(1, 1).is_err());
        b = budget();
        b.repeats = u32::MAX;
        assert!(b.required_calls(u32::MAX, u32::MAX).is_err());
        let mut req = request();
        req.validate().unwrap();
        req.max_total_calls = 11;
        assert!(req.validate().is_err());
        req.max_total_calls = 16;
        req.allow_untrusted_hypotheses = true;
        assert!(req.validate().is_err());
        req.max_total_calls = 17;
        req.validate().unwrap();
    }

    #[test]
    fn inference_settings_reject_invalid_values() {
        for temperature in [f32::NAN, f32::INFINITY, -0.1] {
            let mut p = provenance();
            p.temperature = temperature;
            assert!(p.validate().is_err());
        }
        let mut p = provenance();
        p.max_output_tokens = 0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn cancellation_and_memory_arm_remain_explicit_records() {
        let mut run = experiment();
        run.status = RunStatus::Cancelled;
        roundtrip(&run);
        roundtrip(&ExperimentArm::MemoryRecombination {
            memory_ids: vec!["memory-1".to_string().try_into().unwrap()],
        });
        assert_eq!(
            serde_json::to_value(ExperimentArm::CleanBaseline).unwrap(),
            json!({"kind":"clean_baseline"})
        );
    }
    #[test]
    fn suggestions_require_rationale_and_trace_with_no_review_opt_out() {
        let mut suggestion = SuggestedLucid {
            rationale: claim(),
            supporting_trace: vec![],
        };
        assert!(suggestion.validate().is_err());
        suggestion.supporting_trace.push(TraceEntry {
            stage: "loop-observation".into(),
            claim: "repeated output".into(),
            evidence: Some("turns 1 and 2".into()),
            passed: true,
            note: None,
        });
        suggestion.validate().unwrap();
        let mut value = serde_json::to_value(&suggestion).unwrap();
        value["requires_operator_review"] = json!(false);
        assert!(serde_json::from_value::<SuggestedLucid>(value).is_err());
        suggestion.rationale.text.clear();
        assert!(suggestion.validate().is_err());
    }

    #[test]
    fn lucid_rejects_empty_research_and_repair_rejects_hypothetical_support() {
        let mut req = request();
        req.interventions.clear();
        assert!(req.validate().is_err());
        let mut r = repair();
        r.evidence_for[0].trust = TrustClassification::Hypothetical;
        assert!(r.validate().is_err());
        r.evidence_for[0].trust = TrustClassification::Simulated;
        assert!(r.validate().is_err());
    }

    #[test]
    fn hypotheses_validate_all_generated_content_and_source() {
        let mut h = HypothesisCandidate {
            id: "h".to_string().try_into().unwrap(),
            claim: claim(),
            intervention: request().interventions.remove(0),
            source: provenance(),
        };
        h.validate().unwrap();
        h.claim.text.clear();
        assert!(h.validate().is_err());
        h.claim = claim();
        h.intervention.name.clear();
        assert!(h.validate().is_err());
        h.intervention.name = "clean".into();
        h.intervention.instruction.text.clear();
        assert!(h.validate().is_err());
        h.intervention.instruction = claim();
        h.source.model.clear();
        assert!(h.validate().is_err());
    }
    #[test]
    fn candidate_revision_requires_untrusted_content_and_reason() {
        let request = MemoryRevisionRequest::ReviseCandidate {
            memory_id: "memory-1".to_string().try_into().unwrap(),
            expected_revision: 0,
            content: claim(),
            reason: "Operator proposes revised untrusted content".into(),
        };
        request.validate().unwrap();
        roundtrip(&request);
        let mut invalid = serde_json::to_value(&request).unwrap();
        invalid["reason"] = json!(" ");
        assert!(serde_json::from_value::<MemoryRevisionRequest>(invalid)
            .unwrap()
            .validate()
            .is_err());
        let mut invalid = serde_json::to_value(&request).unwrap();
        invalid["content"]["text"] = json!("");
        assert!(serde_json::from_value::<MemoryRevisionRequest>(invalid)
            .unwrap()
            .validate()
            .is_err());
        let mut invalid = serde_json::to_value(&request).unwrap();
        invalid["content"]["trust"] = json!("operator_validated");
        assert!(serde_json::from_value::<MemoryRevisionRequest>(invalid).is_err());
    }
}
