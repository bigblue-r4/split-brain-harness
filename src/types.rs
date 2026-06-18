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
// Runtime configuration (loaded from config file or env)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub backend: BackendType,
    pub endpoint: String,
    pub model_name: String,
    pub soul_path: String,
    pub api_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Soul container — markdown sections loaded as raw static strings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Soul {
    pub logic_system_prompt: String, // analytical / left-hemisphere stance
    pub creative_system_prompt: String, // affective / right-hemisphere stance
}

// ---------------------------------------------------------------------------
// State transition types — stateless boundaries through the pipeline loop
// ---------------------------------------------------------------------------

pub struct RawInput(pub String);

pub struct LogicReport {
    pub analytical_matrix: String, // raw JSON string from the logic node
}

pub struct CreativeOutput {
    pub raw_response: String, // raw text from the creative node
}

pub struct VerifiedResponse(pub String);

// ---------------------------------------------------------------------------
// Telemetry output schema — mirrors the JSON contract from the soul prompt
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
