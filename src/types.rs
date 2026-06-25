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
    pub timeout_secs: u64,
    /// Print system prompt + payload to stderr before the model call.
    pub dump_prompt: bool,
    /// Print raw model output to stderr before extraction.
    pub dump_raw: bool,
    /// Path to the capability memory JSON file for forge persistence.
    /// None = in-memory only (no cross-session reputation).
    pub memory_path: Option<String>,
    /// Path to the append-only forge audit log (JSONL).
    /// None = no audit logging.
    pub audit_path: Option<String>,
    /// If set, `sbh serve` requires `Authorization: Bearer <serve_key>`.
    /// The serve key is NOT forwarded as the upstream API key.
    pub serve_key: Option<String>,
    /// Max requests per minute per IP for `sbh serve`. Default 60.
    pub serve_rate_limit: u32,
    /// Max request body size in bytes for `sbh serve`. Default 1 MiB.
    pub serve_max_body_bytes: usize,
    /// Path to the append-only session escalation log (JSONL).
    /// Written on every slow-boil escalation event detected by `sbh serve`.
    /// None = events are not persisted.
    pub session_log_path: Option<String>,
    /// Path to operator-supplied context docs (TOML file or directory of TOML files).
    /// Merged with the embedded default corpus and injected into the system prompt.
    /// None = embedded default corpus only.
    pub context_path: Option<String>,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::OpenAiCompat => write!(f, "openai-compat"),
            BackendType::OllamaNative => write!(f, "ollama-native"),
            BackendType::LocalEmbedded => write!(f, "local-embedded"),
            BackendType::Anthropic => write!(f, "anthropic"),
        }
    }
}

impl std::fmt::Display for VerifyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyMode::Deterministic => write!(f, "deterministic"),
            VerifyMode::Llm => write!(f, "llm"),
            VerifyMode::None => write!(f, "none"),
        }
    }
}

// ---------------------------------------------------------------------------
// Soul container
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Soul {
    pub logic_system_prompt: String,
    pub creative_system_prompt: String,
    pub verifier_system_prompt: String,
    pub code_gen_system_prompt: String,
}

// ---------------------------------------------------------------------------
// Telemetry output schema
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AfferentTelemetry {
    pub primary_emotion: String,
    pub emotional_intensity: f32,
    pub structural_tone: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct IntentMatrix {
    pub stated_objective: String,
    pub subtextual_motive: String,
    pub manipulation_risk: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
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

/// Summary of pre-Stage-1 obfuscation detections from the normalizer pass.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ObfuscationReport {
    /// 0.0 = clean input, 1.0 = heavily obfuscated. Threshold ~0.25 for action.
    pub score: f32,
    /// Human-readable list of detected obfuscation events, e.g. ["homoglyph (3)", "base64"].
    pub detections: Vec<String>,
    /// The normalized (deobfuscated) text that was passed to Stage 1.
    pub normalized_input: String,
}

/// Full pipeline output: telemetry + verification + step-level trace.
/// `capability_request` is `None` unless the model emitted one alongside
/// its telemetry (Phase 1 schema — no execution in this release).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HarnessResult {
    pub telemetry: TelemetryResult,
    pub verification: VerificationReport,
    pub trace: Vec<TraceEntry>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capability_request: Option<crate::capability::CapabilityRequest>,
    /// Present when the input required deobfuscation before Stage 1.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub obfuscation: Option<ObfuscationReport>,
}
