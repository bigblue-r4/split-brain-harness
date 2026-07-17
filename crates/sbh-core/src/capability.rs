/// Phase 1 — design-only schema for the Ephemeral Tool Forge.
///
/// The model may emit a capability_request field alongside its telemetry output.
/// No tool is generated or executed in Phase 1. The request is parsed, traced,
/// and stored in HarnessResult. The execution supervisor lives in a later phase.
///
/// Reference: EPHEMERAL_TOOL_FORGE_DESIGN.md
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Model output — what the model emits when it requests a capability
// ---------------------------------------------------------------------------

/// Constraints the model declares on the tool it is requesting.
/// All fields default to the most restrictive value if absent.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConstraints {
    #[serde(default = "default_true")]
    pub no_network: bool,
    #[serde(default = "default_true")]
    pub read_only_input: bool,
    #[serde(default = "default_runtime_ms")]
    pub max_runtime_ms: u64,
    #[serde(default = "default_memory_mb")]
    pub max_memory_mb: u64,
}

fn default_true() -> bool {
    true
}
fn default_runtime_ms() -> u64 {
    1000
}
fn default_memory_mb() -> u64 {
    64
}

impl Default for CapabilityConstraints {
    fn default() -> Self {
        Self {
            no_network: true,
            read_only_input: true,
            max_runtime_ms: 1000,
            max_memory_mb: 64,
        }
    }
}

/// A structured request the model emits when text reasoning is genuinely
/// insufficient for a computational task. The model NEVER generates or runs
/// code — it only describes what it needs.
///
/// The supervisor decides whether to fulfil the request.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequest {
    /// Must be "capability_request" — validated after parse.
    pub kind: String,
    /// Short identifier for the capability class (e.g., "stream_parse_logs").
    pub capability: String,
    /// Human-readable description of the expected input format.
    pub input_contract: String,
    /// Human-readable description of the expected output format.
    pub output_contract: String,
    #[serde(default)]
    pub constraints: CapabilityConstraints,
    /// Why text reasoning alone is insufficient for this task.
    pub reason: String,
}

impl CapabilityRequest {
    /// Returns an error if the request is structurally invalid or exceeds
    /// field-length limits (guards against oversized strings from the model).
    pub fn validate(&self) -> Result<(), String> {
        if self.kind != "capability_request" {
            return Err(format!(
                "kind must be \"capability_request\", got {:?}",
                self.kind
            ));
        }
        if self.capability.trim().is_empty() {
            return Err("capability must not be empty".into());
        }
        if self.reason.trim().is_empty() {
            return Err("reason must not be empty".into());
        }
        if self.capability.len() > crate::input_validation::MAX_CAPABILITY_NAME_BYTES {
            return Err(format!(
                "capability too long: {} bytes (max {})",
                self.capability.len(),
                crate::input_validation::MAX_CAPABILITY_NAME_BYTES
            ));
        }
        if self.reason.len() > crate::input_validation::MAX_REASON_BYTES {
            return Err(format!(
                "reason too long: {} bytes (max {})",
                self.reason.len(),
                crate::input_validation::MAX_REASON_BYTES
            ));
        }
        if self.input_contract.len() > crate::input_validation::MAX_CONTRACT_BYTES {
            return Err(format!(
                "input_contract too long: {} bytes (max {})",
                self.input_contract.len(),
                crate::input_validation::MAX_CONTRACT_BYTES
            ));
        }
        if self.output_contract.len() > crate::input_validation::MAX_CONTRACT_BYTES {
            return Err(format!(
                "output_contract too long: {} bytes (max {})",
                self.output_contract.len(),
                crate::input_validation::MAX_CONTRACT_BYTES
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Supervisor-side types — Phase 2 additions
// ---------------------------------------------------------------------------

/// Measured execution metrics from one tool run (mock or real).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ToolMetrics {
    pub runtime_ms: u64,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub success: bool,
}

/// A policy rule that was violated. Returned as a list from policy::check_request.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyViolation {
    pub rule: String,
    pub detail: String,
}

/// Per-session cost budget enforced by the supervisor.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Budget {
    /// Maximum distinct tool invocations per session.
    pub max_tools_per_session: usize,
    /// Cumulative wall-clock ms across all runs in this session.
    pub max_total_runtime_ms: u64,
    /// Require explicit approval after this many consecutive failures.
    pub require_approval_after_failures: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_tools_per_session: 4,
            max_total_runtime_ms: 30_000,
            require_approval_after_failures: 2,
        }
    }
}

/// Full result returned by the supervisor after processing one CapabilityRequest.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolRunReport {
    /// True if the request passed policy checks and a mock was found.
    pub accepted: bool,
    /// Non-empty when the request was rejected — each entry is one violation.
    pub rejection_reasons: Vec<String>,
    /// Always true for Phase 2 mocks (no generated source to verify yet).
    pub verification_passed: bool,
    /// True if the mock function ran to completion (even if it returned an error).
    pub executed: bool,
    /// Stdout of the mock tool — a JSON string on success, None on rejection.
    pub output: Option<String>,
    pub metrics: ToolMetrics,
    /// Always true after execution — marks the lifecycle as complete.
    pub destroyed: bool,
    /// Set when the run succeeds and memory was updated.
    pub memory_update: Option<CapabilityMemoryRecord>,
}

// ---------------------------------------------------------------------------
// Supervisor-side types — design-only in Phase 1 (manifests, permissions, limits)
// ---------------------------------------------------------------------------

/// Verification steps the supervisor must run before a tool may execute.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationKind {
    StaticAnalysis,
    DependencyScan,
    PolicyCheck,
    UnitTests,
    ResourceEstimate,
}

/// Fine-grained permission set attached to a generated tool manifest.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Permissions {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub filesystem_write: bool,
    /// "none" | "sandbox/input_only" | an explicit allowlisted path
    pub filesystem_read: String,
    #[serde(default)]
    pub process_spawn: bool,
    #[serde(default)]
    pub env_access: bool,
}

/// Hard resource limits enforced by the sandbox runtime.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResourceLimits {
    pub runtime_ms: u64,
    pub memory_mb: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

/// Full manifest created by the tool architect (Phase 2+).
/// In Phase 1 this is a data type only — no generation logic exists yet.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CapabilityManifest {
    pub manifest_version: u32,
    pub capability_id: String,
    pub problem_signature: String,
    pub tool_kind: String,
    pub input_contract: String,
    pub output_contract: String,
    pub permissions: Permissions,
    pub limits: ResourceLimits,
    pub verification_required: Vec<VerificationKind>,
    pub destroy_after_run: bool,
}

/// Capability memory entry — fingerprint stored after a run.
/// The binary is destroyed; only the pattern, constraints, and metrics survive.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CapabilityMemoryRecord {
    pub problem_signature: String,
    pub solution_pattern: String,
    pub input_shape: String,
    pub output_shape: String,
    pub constraints: CapabilityConstraints,
}

// ---------------------------------------------------------------------------
// Extraction adapter — used in harness.rs to wrap TelemetryResult
// ---------------------------------------------------------------------------

/// Wraps the model's full propose-stage output. The telemetry fields are at
/// the top level (flattened) so existing model responses without
/// capability_request still parse unchanged.
#[derive(Debug, Deserialize, Clone)]
pub struct ModelProposalOutput {
    #[serde(flatten)]
    pub telemetry: crate::types::TelemetryResult,
    #[serde(default)]
    pub capability_request: Option<CapabilityRequest>,
    /// Optional one-paragraph, plain-language explanation of the proposer's read.
    /// Top-level (not inside the deny_unknown_fields telemetry sub-structs) so
    /// existing responses without it still parse. Debug/explanation aid only —
    /// it does not affect verification.
    #[serde(default)]
    pub rationale: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_request_json() -> &'static str {
        r#"{
            "kind": "capability_request",
            "capability": "stream_parse_logs",
            "input_contract": "UTF-8 log lines from stdin",
            "output_contract": "JSON array of matching events",
            "constraints": {
                "no_network": true,
                "read_only_input": true,
                "max_runtime_ms": 1000,
                "max_memory_mb": 64
            },
            "reason": "Existing text reasoning is inefficient for repeated regex parsing."
        }"#
    }

    #[test]
    fn capability_request_parses_from_valid_json() {
        let req: CapabilityRequest = serde_json::from_str(valid_request_json()).unwrap();
        assert_eq!(req.kind, "capability_request");
        assert_eq!(req.capability, "stream_parse_logs");
        assert!(req.constraints.no_network);
        assert_eq!(req.constraints.max_runtime_ms, 1000);
    }

    #[test]
    fn capability_request_validates_kind() {
        let mut req: CapabilityRequest = serde_json::from_str(valid_request_json()).unwrap();
        req.kind = "wrong_kind".into();
        assert!(req.validate().is_err());
    }

    #[test]
    fn capability_request_validates_empty_capability() {
        let mut req: CapabilityRequest = serde_json::from_str(valid_request_json()).unwrap();
        req.capability = "  ".into();
        assert!(req.validate().is_err());
    }

    #[test]
    fn capability_request_validates_empty_reason() {
        let mut req: CapabilityRequest = serde_json::from_str(valid_request_json()).unwrap();
        req.reason = String::new();
        assert!(req.validate().is_err());
    }

    #[test]
    fn capability_request_constraints_default_restrictive() {
        let json = r#"{
            "kind": "capability_request",
            "capability": "test",
            "input_contract": "x",
            "output_contract": "y",
            "reason": "z"
        }"#;
        let req: CapabilityRequest = serde_json::from_str(json).unwrap();
        assert!(
            req.constraints.no_network,
            "default must be no_network=true"
        );
        assert!(
            req.constraints.read_only_input,
            "default must be read_only=true"
        );
        assert_eq!(req.constraints.max_runtime_ms, 1000);
        assert_eq!(req.constraints.max_memory_mb, 64);
    }

    #[test]
    fn model_proposal_output_parses_telemetry_only() {
        let json = r#"{
            "affective_telemetry": {
                "primary_emotion": "neutral",
                "emotional_intensity": 0.1,
                "structural_tone": ["analytical"]
            },
            "intent_matrix": {
                "stated_objective": "test",
                "subtextual_motive": "test",
                "manipulation_risk": "low"
            },
            "cognitive_state": {
                "urgency_vector": 0.0,
                "coherence_rating": 0.95
            }
        }"#;
        let output: ModelProposalOutput = serde_json::from_str(json).unwrap();
        assert!(
            output.capability_request.is_none(),
            "capability_request must be absent when not emitted"
        );
        assert_eq!(output.telemetry.intent_matrix.manipulation_risk, "low");
    }

    #[test]
    fn model_proposal_output_parses_telemetry_with_capability_request() {
        let json = r#"{
            "affective_telemetry": {
                "primary_emotion": "neutral",
                "emotional_intensity": 0.1,
                "structural_tone": ["analytical"]
            },
            "intent_matrix": {
                "stated_objective": "parse 10GB log file",
                "subtextual_motive": "efficiency",
                "manipulation_risk": "low"
            },
            "cognitive_state": {
                "urgency_vector": 0.2,
                "coherence_rating": 0.95
            },
            "capability_request": {
                "kind": "capability_request",
                "capability": "stream_parse_logs",
                "input_contract": "UTF-8 log lines",
                "output_contract": "JSON events",
                "constraints": {
                    "no_network": true,
                    "read_only_input": true,
                    "max_runtime_ms": 2000,
                    "max_memory_mb": 128
                },
                "reason": "10GB file cannot be reasoned over line-by-line in a single context window."
            }
        }"#;
        let output: ModelProposalOutput = serde_json::from_str(json).unwrap();
        let req = output.capability_request.unwrap();
        assert_eq!(req.capability, "stream_parse_logs");
        assert_eq!(req.constraints.max_memory_mb, 128);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn verification_kind_roundtrips() {
        let kinds = vec![
            VerificationKind::StaticAnalysis,
            VerificationKind::DependencyScan,
            VerificationKind::PolicyCheck,
            VerificationKind::UnitTests,
            VerificationKind::ResourceEstimate,
        ];
        for k in kinds {
            let s = serde_json::to_string(&k).unwrap();
            let back: VerificationKind = serde_json::from_str(&s).unwrap();
            assert_eq!(k, back);
        }
    }
}
