use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub enum BackendType {
    #[serde(rename = "openai-compat")]
    OpenAiCompat,
    #[serde(rename = "ollama-native")]
    OllamaNative,
    #[serde(rename = "local-embedded")]
    LocalEmbedded,
    #[serde(rename = "anthropic")]
    Anthropic,
}

// ---------------------------------------------------------------------------
// Verification mode
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
pub enum VerifyMode {
    /// Deterministic consistency checks only — no extra LLM call (default).
    #[serde(rename = "deterministic")]
    #[default]
    Deterministic,
    /// Deterministic checks + a second LLM call against the verifier soul prompt.
    #[serde(rename = "llm")]
    Llm,
    /// Skip verification entirely.
    #[serde(rename = "none")]
    None,
}

// ---------------------------------------------------------------------------
// Runtime configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub backend: BackendType,
    pub endpoint: String,
    pub model_name: String,
    pub soul_path: String,
    pub api_key: Option<String>,
    pub verify_mode: VerifyMode,
}

// ---------------------------------------------------------------------------
// Soul container
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Soul {
    pub logic_system_prompt: String,
    pub creative_system_prompt: String,
    pub verifier_system_prompt: String,
}

// ---------------------------------------------------------------------------
// Internal pipeline state types
// ---------------------------------------------------------------------------

pub struct LogicReport {
    pub analytical_matrix: String,
}

// ---------------------------------------------------------------------------
// Telemetry output schema
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AfferentTelemetry {
    pub primary_emotion: String,
    pub emotional_intensity: f32,
    pub structural_tone: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IntentMatrix {
    pub stated_objective: String,
    pub subtextual_motive: String,
    pub manipulation_risk: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CognitiveState {
    pub urgency_vector: f32,
    pub coherence_rating: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TelemetryResult {
    pub affective_telemetry: AfferentTelemetry,
    pub intent_matrix: IntentMatrix,
    pub cognitive_state: CognitiveState,
}

// ---------------------------------------------------------------------------
// Verification layer
// ---------------------------------------------------------------------------

/// One step in the analysis pipeline — propose, deterministic check, or LLM verify.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TraceEntry {
    pub stage: String,
    pub claim: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result of the verification stage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VerificationReport {
    pub passed: bool,
    pub consistency_flags: Vec<String>,
    pub unsupported_claims: Vec<String>,
    pub assumptions: Vec<String>,
    pub unresolved: Vec<String>,
    pub confidence: f32,
    /// When true, confidence is below threshold — caller should pause and ask
    /// for clarification rather than acting on the result.
    pub stop_and_ask: bool,
}

/// Full pipeline output: telemetry + verification + step-level trace.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HarnessResult {
    pub telemetry: TelemetryResult,
    pub verification: VerificationReport,
    pub trace: Vec<TraceEntry>,
}
