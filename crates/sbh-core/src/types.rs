use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
pub enum BackendType {
    #[serde(rename = "openai-compat")]
    OpenAiCompat,
    #[serde(rename = "ollama-native")]
    #[default]
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
    /// Deterministic checks + LLM verifier + a third adjudicator LLM call when the
    /// disagreement structure matches a high-risk injection fingerprint.
    /// Inspired by ReConcile (ACL) multi-model consensus and DiscoUQ structured
    /// disagreement scoring.
    #[serde(rename = "reconcile")]
    Reconcile,
    /// Skip verification entirely.
    #[serde(rename = "none")]
    None,
}

// ---------------------------------------------------------------------------
// Arbitrator mode (v1.5 active reconciliation)
// ---------------------------------------------------------------------------

/// Controls the post-verify reconciliation loop.
#[derive(Debug, Deserialize, Clone, Default, PartialEq, Eq)]
pub enum ArbitratorMode {
    /// No refinement loop — one-shot propose→verify→gate (pre-v1.5 behavior).
    #[serde(rename = "off")]
    Off,
    /// Bounded refinement loop adjudicated by deterministic rules (no extra LLM
    /// call). Default. An `Llm` variant is reserved for v2.
    #[serde(rename = "rules")]
    #[default]
    Rules,
}

impl std::fmt::Display for ArbitratorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArbitratorMode::Off => write!(f, "off"),
            ArbitratorMode::Rules => write!(f, "rules"),
        }
    }
}

/// Controls the Devil's-Advocate / debate stage (phase E) — an adversarial
/// third LLM pass that argues the proposer's read is wrong. It can only ever
/// *raise* caution, never lower it.
#[derive(Debug, Deserialize, Clone, Default, PartialEq, Eq)]
pub enum AdvocateMode {
    /// Never run the advocate. Default — zero added latency/cost.
    #[serde(rename = "off")]
    #[default]
    Off,
    /// Run only on high-stakes inputs (a deterministic gate: high manipulation
    /// risk, a risky tool surface, or a capability request). One extra LLM call.
    #[serde(rename = "high_stakes")]
    HighStakes,
    /// Run on every analysis. One extra LLM call per request — use deliberately.
    #[serde(rename = "always")]
    Always,
}

impl std::fmt::Display for AdvocateMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdvocateMode::Off => write!(f, "off"),
            AdvocateMode::HighStakes => write!(f, "high_stakes"),
            AdvocateMode::Always => write!(f, "always"),
        }
    }
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
    /// Sampling temperature forwarded to the backend. Default 0.1 — the
    /// verification gates assume near-deterministic proposer output.
    /// Values above 0.5 trigger a randomness discount on verifier confidence
    /// so borderline stop_and_ask decisions stay consistent across runs.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
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

    // --- v1.5 active reconciliation ---
    /// Adjudication mode for the post-verify refinement loop. Default `Rules`.
    /// `Off` restores one-shot propose→verify behavior.
    #[serde(default)]
    pub arbitrator: ArbitratorMode,
    /// Maximum propose→verify iterations, including the first. Default 2.
    /// 1 (or `arbitrator = off`) disables refinement.
    #[serde(default = "default_refine_max_iters")]
    pub refine_max_iters: usize,
    /// Confidence at/above which the arbitrator accepts and stops refining.
    /// Default 0.4 (matches the stop_and_ask threshold).
    #[serde(default = "default_stop_and_ask_threshold")]
    pub refine_confidence_target: f32,
    /// Gate threshold: `stop_and_ask` fires below this confidence. Default 0.4.
    /// Promotes the previously hardcoded verifier constant to config.
    #[serde(default = "default_stop_and_ask_threshold")]
    pub stop_and_ask_threshold: f32,
    /// Path to the append-only confidence-calibration store (JSONL).
    /// None = calibration features are not logged.
    pub calibration_path: Option<String>,
    /// Ask the proposer to emit a natural-language `rationale` paragraph.
    /// Default **false**: the extra generation hurts JSON reliability and latency
    /// on small local models (the default llama3.2:3b backend). Enable it with a
    /// capable backend when the explanation is worth the cost.
    #[serde(default)]
    pub request_rationale: bool,
    /// Path to Formal-stage rule domains (phase F): a single `.toml` file or a
    /// directory of them. None = the Formal stage is a no-op (no behavior change).
    pub formal_rules_path: Option<String>,
    /// Devil's-Advocate / debate stage (phase E). Default `Off` (no extra LLM
    /// call). `HighStakes` gates on a deterministic risk predicate; `Always` runs
    /// every request.
    #[serde(default)]
    pub advocate_mode: AdvocateMode,
    /// Per-request LLM-call ceiling (phase E.2). `None` = unlimited (calls are
    /// still counted). When set, calls beyond the limit are refused — a hard stop
    /// on call stacking across refinement, verifier, and advocate.
    pub max_llm_calls_per_request: Option<usize>,
}

fn default_temperature() -> f32 {
    0.1
}

fn default_refine_max_iters() -> usize {
    2
}

fn default_stop_and_ask_threshold() -> f32 {
    0.4
}

impl Default for Config {
    /// Canonical defaults — the same values `build_config` falls back to when no
    /// env var or config.toml entry is present. Lets call sites (and tests) write
    /// `Config { verify_mode: ..., ..Default::default() }` instead of a full literal.
    fn default() -> Self {
        Config {
            backend: BackendType::OllamaNative,
            endpoint: "http://localhost:11434".into(),
            model_name: "llama3.2:3b".into(),
            soul_path: String::new(),
            api_key: None,
            verify_mode: VerifyMode::Deterministic,
            timeout_secs: 120,
            temperature: default_temperature(),
            dump_prompt: false,
            dump_raw: false,
            memory_path: None,
            audit_path: None,
            serve_key: None,
            serve_rate_limit: 60,
            serve_max_body_bytes: 1_048_576,
            session_log_path: None,
            context_path: None,
            arbitrator: ArbitratorMode::Rules,
            refine_max_iters: default_refine_max_iters(),
            refine_confidence_target: default_stop_and_ask_threshold(),
            stop_and_ask_threshold: default_stop_and_ask_threshold(),
            calibration_path: None,
            request_rationale: false,
            formal_rules_path: None,
            advocate_mode: AdvocateMode::Off,
            max_llm_calls_per_request: None,
        }
    }
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
            VerifyMode::Reconcile => write!(f, "reconcile"),
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
    /// Adversarial red-team prompt for the Devil's-Advocate stage (phase E).
    /// Optional — empty when the soul has no `[ADVOCATE_SYSTEM_PROMPT]` section,
    /// in which case the advocate stage is skipped even if enabled.
    pub advocate_system_prompt: String,
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

/// Coercion risk directed at the AI system. Typed, but **tolerant on the parse
/// boundary**: the model must emit "low" | "medium" | "high", yet any other value
/// deserializes to `Unknown(raw)` instead of failing the whole parse — preserving
/// the raw string so the `manipulation-risk-value` check can flag it. Serializes
/// back to the same lowercase strings (round-trip preserved), so downstream
/// consumers that read `manipulation_risk` see no change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
    Unknown(String),
}

impl Risk {
    pub fn as_str(&self) -> &str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::Unknown(s) => s,
        }
    }
    /// True unless the model emitted an unrecognized risk value.
    pub fn is_recognized(&self) -> bool {
        !matches!(self, Risk::Unknown(_))
    }
}

impl From<&str> for Risk {
    fn from(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "low" => Risk::Low,
            "medium" => Risk::Medium,
            "high" => Risk::High,
            _ => Risk::Unknown(s.to_string()),
        }
    }
}
impl From<String> for Risk {
    fn from(s: String) -> Self {
        Risk::from(s.as_str())
    }
}
impl std::fmt::Display for Risk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl Serialize for Risk {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for Risk {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Risk::from(String::deserialize(d)?))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct IntentMatrix {
    pub stated_objective: String,
    pub subtextual_motive: String,
    pub manipulation_risk: Risk,
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

/// Tool-use risk of an input's intent (phase C). Derived **deterministically** —
/// from input patterns cross-checked against the model's `capability_request` —
/// never from the model's own self-report. Answers "would satisfying this intent
/// touch code execution / the web / the filesystem / the network / a shell?".
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ToolRisk {
    pub code_execution: bool,
    pub web_access: bool,
    pub file_write: bool,
    pub network: bool,
    pub shell: bool,
    /// How this was derived: "deterministic" (input patterns) and/or "capability_request".
    pub sources: Vec<String>,
    /// The specific markers that matched, for transparency.
    pub markers: Vec<String>,
}

impl ToolRisk {
    /// True if the intent touches any tool surface.
    pub fn any(&self) -> bool {
        self.code_execution || self.web_access || self.file_write || self.network || self.shell
    }
}

// ---------------------------------------------------------------------------
// Formal-ish verification (phase F)
// ---------------------------------------------------------------------------

/// A single deterministic predicate violation raised by the Formal stage.
/// `rule_id` is stable across runs (feeds `fired_checks` for HITL tuning, D).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FormalViolation {
    pub rule_id: String,
    /// Human-readable explanation of what the rule requires and why it fired.
    pub message: String,
    /// Rule-declared severity. `High` violations escalate the gate (stop_and_ask).
    pub severity: Risk,
}

/// Result of running the Formal stage's deterministic predicate engine over the
/// loaded rule domains. Present on `HarnessResult` only when rules are configured
/// AND at least one rule's triggers matched — so with no rules the stage is a
/// no-op and behavior is unchanged.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FormalReport {
    /// Rule domains whose triggers matched and were evaluated.
    pub domains: Vec<String>,
    /// Rule IDs that were evaluated (triggers matched), whether or not they violated.
    pub checked: Vec<String>,
    /// Violations found — empty means all evaluated rules held.
    pub violations: Vec<FormalViolation>,
    /// True when there were no violations among the evaluated rules.
    pub passed: bool,
}

impl FormalReport {
    /// True if any violation is `High` severity — the harness escalates the gate.
    pub fn has_high_severity(&self) -> bool {
        self.violations
            .iter()
            .any(|v| matches!(v.severity, Risk::High))
    }
}

// ---------------------------------------------------------------------------
// Devil's-Advocate / debate (phase E)
// ---------------------------------------------------------------------------

/// The advocate's adversarial reading of the input. Present on `HarnessResult`
/// only when the advocate stage actually ran (gate passed and a call was made).
/// By design the advocate can only ever *raise* caution — a `Benign` verdict is
/// informational and never clears a flag or lifts the gate.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdvocateReport {
    /// "attack" | "benign" | "unsure" — the advocate's verdict.
    pub verdict: String,
    /// Advocate's confidence in its verdict, 0.0–1.0.
    pub confidence: f32,
    /// One-sentence strongest argument that the input is manipulative.
    pub argument: String,
    /// True when the advocate dissents with enough confidence to escalate the
    /// gate (verdict "attack" above the dissent threshold).
    pub dissented: bool,
    /// Why the advocate ran — the gate signal(s) that triggered it.
    pub gate_reason: Vec<String>,
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

/// Structured analysis of how the verification layer disagrees with the proposer.
///
/// Inspired by DiscoUQ (structured inter-agent disagreement scoring): not all flag
/// counts are equal. Two flags from the same analytical domain suggest a single
/// root cause; flags spread across domains suggest a broader attack surface. The
/// injection fingerprint fires when the flag combination matches the canonical
/// manipulation-evasion pattern (adversarial tone + urgency both present while
/// manipulation_risk is asserted low).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct DisagreementScore {
    /// Number of deterministic consistency checks that fired.
    pub flag_count: usize,
    /// Fraction of total checks that fired (0.0–1.0).
    pub flag_density: f32,
    /// Number of distinct analytical dimensions with at least one flag.
    /// Dimensions: affective, tone, urgency, coherence, risk-value, risk-signal.
    pub dimension_spread: usize,
    /// True when the flag set matches the canonical injection-evasion fingerprint:
    /// adversarial/coercive tone + high urgency both flagging against a low
    /// manipulation_risk assertion. This pattern indicates the proposer was deceived
    /// by a payload designed to appear benign while exerting coercive pressure.
    pub injection_fingerprint: bool,
    /// Confidence derived from disagreement structure (replaces flat flag-count penalty).
    /// Uses density and fingerprint match instead of a simple per-flag discount.
    pub adjusted_confidence: f32,
    /// Present when Reconcile mode ran — summary of the adjudicator's verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconcile_verdict: Option<String>,
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
    /// Structured disagreement analysis (DiscoUQ-inspired). Always populated.
    pub disagreement: DisagreementScore,
    /// When true, confidence is below threshold — caller should pause and ask
    /// for clarification rather than acting on the result.
    pub stop_and_ask: bool,
    /// IDs of the deterministic checks that fired (for HITL weight-tuning, D).
    /// Additive; empty when no checks fired or verification was skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fired_checks: Vec<String>,
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

// ---------------------------------------------------------------------------
// Active reconciliation (v1.5): refinement loop + arbitrator
// ---------------------------------------------------------------------------

/// The arbitrator's verdict over a set of propose→verify iterations.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum ArbiterVerdict {
    /// A revision cleared the bar — surface it as the result.
    #[serde(rename = "accept")]
    Accept,
    /// Disagreement persists and budget remains — propose again with feedback.
    #[serde(rename = "re_refine")]
    ReRefine,
    /// Budget exhausted without resolution — surface the best attempt and force
    /// stop_and_ask.
    #[serde(rename = "escalate")]
    Escalate,
}

impl std::fmt::Display for ArbiterVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArbiterVerdict::Accept => write!(f, "accept"),
            ArbiterVerdict::ReRefine => write!(f, "re_refine"),
            ArbiterVerdict::Escalate => write!(f, "escalate"),
        }
    }
}

/// The arbitrator's decision: which iteration to surface and why.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArbiterDecision {
    pub verdict: ArbiterVerdict,
    /// Index into `RefinementTrace::iterations` chosen as the final result.
    pub chosen_iteration: usize,
    pub reasoning: String,
}

/// Summary of one propose→verify pass inside the refinement loop.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RefinementIteration {
    pub iteration: usize,
    pub confidence: f32,
    pub passed: bool,
    pub stop_and_ask: bool,
    pub flag_count: usize,
}

/// The full refinement record — present only when the arbitrator loop ran.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RefinementTrace {
    pub iterations: Vec<RefinementIteration>,
    pub decision: ArbiterDecision,
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
    /// Present when the active-reconciliation loop ran (arbitrator = rules).
    /// Absent (skipped) when arbitrator = off or only a single pass ran.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refinement: Option<RefinementTrace>,
    /// Present when the input's intent touches a tool surface (phase C).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_risk: Option<ToolRisk>,
    /// Present when Formal rules are configured and at least one rule's triggers
    /// matched (phase F). Absent when no rules are loaded or none applied.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub formal: Option<FormalReport>,
    /// Present when the Devil's-Advocate stage ran (phase E) — i.e. advocate_mode
    /// is enabled and (for HighStakes) the gate passed. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub advocate: Option<AdvocateReport>,
    /// Number of LLM calls made servicing this request (phase E.2). Feeds the
    /// `sbh_llm_calls_total` metric and surfaces call stacking in the trace.
    #[serde(default)]
    pub llm_calls: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_parses_canonical_values_case_insensitively() {
        assert_eq!(Risk::from("low"), Risk::Low);
        assert_eq!(Risk::from("HIGH"), Risk::High);
        assert_eq!(Risk::from("  Medium "), Risk::Medium);
    }

    #[test]
    fn risk_captures_unknown_values_instead_of_failing() {
        let r = Risk::from("banana");
        assert_eq!(r, Risk::Unknown("banana".into()));
        assert!(!r.is_recognized());
        assert!(Risk::Low.is_recognized());
    }

    #[test]
    fn risk_deserialize_is_tolerant_and_round_trips() {
        // A bad value must NOT fail the whole telemetry parse.
        let r: Risk = serde_json::from_str("\"weird\"").unwrap();
        assert_eq!(r, Risk::Unknown("weird".into()));
        // Canonical values serialize back to the same lowercase strings.
        assert_eq!(serde_json::to_string(&Risk::High).unwrap(), "\"high\"");
        assert_eq!(serde_json::to_string(&r).unwrap(), "\"weird\"");
    }

    #[test]
    fn intent_matrix_tolerates_unknown_risk() {
        let json = r#"{"stated_objective":"o","subtextual_motive":"m","manipulation_risk":"nope"}"#;
        let im: IntentMatrix = serde_json::from_str(json).unwrap();
        assert_eq!(im.manipulation_risk, Risk::Unknown("nope".into()));
    }
}
