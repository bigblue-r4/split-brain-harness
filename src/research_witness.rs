//! Opt-in research evaluation, separate from the external decision-log witness.
//! Provider factories are trusted adapters and must reset context for every call.
use crate::{
    dream::{ContextId, GenerationInput, RunId, TestId, TestOutcome},
    dream_engine::{EngineSession, InferenceSettings, ProviderProfile},
};
use serde::{Deserialize, Serialize};

/// Controller-approved projection. No public constructor or deserialization:
/// only generation input, final answer and evaluator-owned oracle can enter it.
/// It carries no memory capability, IDs, content, permits or generation history.
///
/// ```compile_fail
/// use split_brain_harness::research_witness::ApprovedEvaluationInput;
/// fn forbidden(input: ApprovedEvaluationInput) { input.memory_reader(); }
/// ```
/// ```compile_fail
/// use split_brain_harness::research_witness::ApprovedEvaluationInput;
/// let forged: ApprovedEvaluationInput = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Debug, Serialize)]
pub struct ApprovedEvaluationInput {
    input: GenerationInput,
    final_answer: String,
    expected_response: Option<String>,
}
impl ApprovedEvaluationInput {
    pub(crate) fn new(
        input: GenerationInput,
        final_answer: String,
        expected_response: Option<String>,
    ) -> Self {
        Self {
            input,
            final_answer,
            expected_response,
        }
    }
    pub fn generation_input(&self) -> &GenerationInput {
        &self.input
    }
    pub fn final_answer(&self) -> &str {
        &self.final_answer
    }
    pub fn expected_response(&self) -> Option<&str> {
        self.expected_response.as_deref()
    }
}

/// A separate provider boundary from generation. The controller sends only the
/// approved projection to its fresh engine, never a serialized research request.
pub trait FreshWitnessProvider: Send + Sync {
    fn profile(&self) -> ProviderProfile;
    fn open(
        &self,
        context_id: &ContextId,
        settings: &InferenceSettings,
    ) -> Result<EngineSession, String>;
}

/// Untrusted independent observation. Outcome is a deterministic projection of
/// the effective verdict, never authority for repairs or memory promotion.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessAssessment {
    pub outcome: TestOutcome,
    pub witness: sbh_core::witness_schema::WitnessReport,
}

/// Kept separately from TestResult and never used to promote or produce repairs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessEvaluation {
    pub evidence_run_id: RunId,
    pub test_id: TestId,
    pub arm_index: usize,
    pub repeat: u32,
    pub sample: u32,
    pub context_id: ContextId,
    pub assessment: WitnessAssessment,
    /// Agreement with controller exact-oracle scoring of the model's final
    /// answer, not proof. None when either evaluation is inconclusive.
    pub agreement: Option<bool>,
}
