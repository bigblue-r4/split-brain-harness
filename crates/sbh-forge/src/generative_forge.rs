/// Phase 3 of the Ephemeral Tool Forge — source generation and static analysis.
///
/// Takes a `CapabilityRequest`, calls the inference engine to produce a Rust
/// function with inline tests, then runs static analysis and verifies tests
/// are present.  Does NOT execute the code — that is Phase 4 (`wasm_forge`).
///
/// **Status**: production-quality, full test coverage.
/// **Requires**: a configured inference backend (SBH_BACKEND / SBH_API_KEY).
/// **CLI entry point**: `sbh forge "<capability>" "<input>"`
///
/// Pipeline:
///   input validation → policy check → code generation →
///   static analysis → test presence check → memory update
use std::time::Instant;

use crate::code_gen::{CodeGenerator, GeneratedTool};
use crate::tool_memory::CapabilityMemory;
use sbh_core::capability::{Budget, CapabilityMemoryRecord, CapabilityRequest, ToolMetrics};
use sbh_core::input_validation;
use sbh_core::types::Soul;
use sbh_llm::InferenceEngine;
use sbh_safety::policy::{self, PolicyState};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Report type — richer than Phase 2 ToolRunReport
// ---------------------------------------------------------------------------

/// Full result of one generative forge pass.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GenerativeReport {
    /// True if the request passed all pre-generation checks.
    pub accepted: bool,
    /// Non-empty when the request was rejected before generation.
    pub rejection_reasons: Vec<String>,
    /// True if static analysis passed AND tests are present.
    /// False when generation succeeds but output is unsafe or missing tests.
    pub verification_passed: bool,
    /// Phase 3 never executes against real data.
    pub executed: bool,
    /// The generated tool (present when generation succeeded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_tool: Option<GeneratedTool>,
    /// Set when the LLM call or code block extraction failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_error: Option<String>,
    pub metrics: ToolMetrics,
    /// True: source is not persisted after this call.
    pub destroyed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_update: Option<CapabilityMemoryRecord>,
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// Phase 3 supervisor. One instance per session.
pub struct GenerativeForge<'e> {
    budget: Budget,
    state: PolicyState,
    pub memory: CapabilityMemory,
    engine: &'e dyn InferenceEngine,
    soul: Soul,
    session_log: Vec<GenerativeReport>,
}

impl<'e> GenerativeForge<'e> {
    pub fn new(engine: &'e dyn InferenceEngine, soul: Soul) -> Self {
        Self {
            budget: Budget::default(),
            state: PolicyState::default(),
            memory: CapabilityMemory::new(),
            engine,
            soul,
            session_log: vec![],
        }
    }

    pub fn with_budget(budget: Budget, engine: &'e dyn InferenceEngine, soul: Soul) -> Self {
        Self {
            budget,
            state: PolicyState::default(),
            memory: CapabilityMemory::new(),
            engine,
            soul,
            session_log: vec![],
        }
    }

    /// Every call is recorded in the session log regardless of outcome.
    pub fn audit(&self) -> &[GenerativeReport] {
        &self.session_log
    }

    /// Process one CapabilityRequest.
    pub async fn handle(&mut self, req: &CapabilityRequest, input: &str) -> GenerativeReport {
        let report = self.handle_inner(req, input).await;
        self.session_log.push(report.clone());
        report
    }

    async fn handle_inner(&mut self, req: &CapabilityRequest, input: &str) -> GenerativeReport {
        // Input validation
        if let Err(e) = input_validation::validate_forge_input(input) {
            return rejected(vec![format!("input validation: {e}")]);
        }
        if let Err(e) = input_validation::validate_capability_fields(req) {
            return rejected(vec![format!("capability field validation: {e}")]);
        }

        // Budget check
        if let Some(reason) = self.state.budget_exceeded(&self.budget) {
            return rejected(vec![reason]);
        }

        // Policy checks
        let violations = policy::check_request(req);
        if !violations.is_empty() {
            return rejected(violations.into_iter().map(|v| v.detail).collect());
        }

        // Code generation
        let generator = CodeGenerator::new(self.engine, &self.soul);
        let start = Instant::now();
        let gen_result = generator.generate(req).await;
        let generation_ms = start.elapsed().as_millis() as u64;

        let generated: GeneratedTool = match gen_result {
            Ok(tool) => tool,
            Err(e) => {
                let metrics = ToolMetrics {
                    runtime_ms: generation_ms,
                    input_bytes: input.len(),
                    output_bytes: 0,
                    success: false,
                };
                self.state.record_run(&metrics);
                return GenerativeReport {
                    accepted: true,
                    rejection_reasons: vec![],
                    verification_passed: false,
                    executed: false,
                    generated_tool: None,
                    generation_error: Some(format!("code generation failed: {e}")),
                    metrics,
                    destroyed: true,
                    memory_update: None,
                };
            }
        };

        // Verification: static analysis must pass AND at least 2 tests present
        let verification_passed = generated.static_analysis.passed && generated.tests_included;

        let metrics = ToolMetrics {
            runtime_ms: generation_ms,
            input_bytes: input.len(),
            output_bytes: generated.source.len(),
            success: verification_passed,
        };
        self.state.record_run(&metrics);

        // Memory update on success
        let memory_update = if verification_passed {
            let signature = CapabilityMemory::derive_signature(req);
            let record = CapabilityMemoryRecord {
                problem_signature: signature,
                solution_pattern: format!("generated:{}", req.capability),
                input_shape: shape_token(&req.input_contract),
                output_shape: shape_token(&req.output_contract),
                constraints: req.constraints.clone(),
            };
            self.memory.upsert(record.clone(), &metrics);
            Some(record)
        } else {
            None
        };

        GenerativeReport {
            accepted: true,
            rejection_reasons: vec![],
            verification_passed,
            executed: false,
            generated_tool: Some(generated),
            generation_error: None,
            metrics,
            destroyed: true,
            memory_update,
        }
    }

    pub fn tools_invoked(&self) -> usize {
        self.state.tools_invoked
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn rejected(reasons: Vec<String>) -> GenerativeReport {
    GenerativeReport {
        accepted: false,
        rejection_reasons: reasons,
        verification_passed: false,
        executed: false,
        generated_tool: None,
        generation_error: None,
        metrics: ToolMetrics::default(),
        destroyed: false,
        memory_update: None,
    }
}

fn shape_token(contract: &str) -> String {
    contract
        .split_whitespace()
        .take(3)
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sbh_core::capability::CapabilityConstraints;
    use sbh_safety::soul;

    // --- Mock engine helpers ---

    struct MockEngine {
        response: String,
    }

    #[async_trait]
    impl sbh_llm::InferenceEngine for MockEngine {
        async fn generate(&self, _sys: &str, _prompt: &str) -> Result<String, String> {
            Ok(self.response.clone())
        }
    }

    struct ErrorEngine;

    #[async_trait]
    impl sbh_llm::InferenceEngine for ErrorEngine {
        async fn generate(&self, _sys: &str, _prompt: &str) -> Result<String, String> {
            Err("backend unavailable".into())
        }
    }

    fn clean_req(cap: &str) -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: cap.into(),
            input_contract: "utf8 text lines".into(),
            output_contract: "json object".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning insufficient for this task".into(),
        }
    }

    const CLEAN_RUST_RESPONSE: &str = r#"```rust
pub fn run(input: &str) -> Result<String, String> {
    let count = input.split_whitespace().count();
    Ok(format!("{\"word_count\":{}}", count))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_words() {
        assert!(run("hello world").unwrap().contains("2"));
    }
    #[test]
    fn empty_input() {
        assert!(run("").unwrap().contains("0"));
    }
}
```"#;

    const UNSAFE_RUST_RESPONSE: &str = r#"```rust
pub fn run(input: &str) -> Result<String, String> {
    unsafe { let _ = 0; }
    Ok(format!("{\"word_count\":{}}", input.len()))
}
#[test]
fn t1() {}
#[test]
fn t2() {}
```"#;

    const NO_TESTS_RESPONSE: &str = r#"```rust
pub fn run(input: &str) -> Result<String, String> {
    Ok(input.to_string())
}
```"#;

    const NO_CODE_BLOCK_RESPONSE: &str = "Here is my analysis but no code block.";

    // --- Acceptance paths ---

    #[tokio::test]
    async fn accepts_clean_request_and_clean_code() {
        let engine = MockEngine {
            response: CLEAN_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let req = clean_req("word_count");
        let report = forge.handle(&req, "some input").await;

        assert!(report.accepted);
        assert!(
            report.verification_passed,
            "clean code + 2 tests should pass"
        );
        assert!(!report.executed, "Phase 3 never executes");
        assert!(report.generated_tool.is_some());
        assert!(report.generation_error.is_none());
    }

    // --- Rejection paths (before generation) ---

    #[tokio::test]
    async fn rejects_network_access_request() {
        let engine = MockEngine {
            response: CLEAN_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let mut req = clean_req("fetch_url");
        req.constraints.no_network = false;
        let report = forge.handle(&req, "input").await;
        assert!(!report.accepted);
        assert!(report
            .rejection_reasons
            .iter()
            .any(|r| r.contains("no_network")));
    }

    #[tokio::test]
    async fn rejects_when_budget_exhausted() {
        let budget = Budget {
            max_tools_per_session: 1,
            ..Budget::default()
        };
        let engine = MockEngine {
            response: CLEAN_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::with_budget(budget, &engine, soul);
        forge.handle(&clean_req("word_count"), "a").await;
        let report = forge.handle(&clean_req("word_count"), "b").await;
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("session tool limit"));
    }

    #[tokio::test]
    async fn rejects_oversized_input() {
        let engine = MockEngine {
            response: CLEAN_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let big = "x".repeat(sbh_core::input_validation::MAX_FORGE_INPUT_BYTES + 1);
        let report = forge.handle(&clean_req("word_count"), &big).await;
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("input validation"));
    }

    // --- Verification failure paths (after generation) ---

    #[tokio::test]
    async fn fails_verification_on_static_violation() {
        let engine = MockEngine {
            response: UNSAFE_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let report = forge.handle(&clean_req("word_count"), "input").await;
        assert!(report.accepted, "request was accepted");
        assert!(
            !report.verification_passed,
            "unsafe code must fail verification"
        );
        let tool = report.generated_tool.unwrap();
        assert!(!tool.static_analysis.passed);
        assert!(tool
            .static_analysis
            .violations
            .iter()
            .any(|v| v.kind == "unsafe_code"));
    }

    #[tokio::test]
    async fn fails_verification_on_missing_tests() {
        let engine = MockEngine {
            response: NO_TESTS_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let report = forge.handle(&clean_req("word_count"), "input").await;
        assert!(report.accepted);
        assert!(
            !report.verification_passed,
            "code without tests must fail verification"
        );
        let tool = report.generated_tool.unwrap();
        assert!(!tool.tests_included);
    }

    #[tokio::test]
    async fn generation_error_when_no_code_block() {
        let engine = MockEngine {
            response: NO_CODE_BLOCK_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let report = forge.handle(&clean_req("word_count"), "input").await;
        assert!(report.accepted, "request was valid");
        assert!(!report.verification_passed);
        assert!(report.generated_tool.is_none());
        assert!(report.generation_error.is_some());
    }

    #[tokio::test]
    async fn generation_error_when_backend_fails() {
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&ErrorEngine, soul);
        let report = forge.handle(&clean_req("word_count"), "input").await;
        assert!(report.accepted);
        assert!(!report.verification_passed);
        assert!(report
            .generation_error
            .as_deref()
            .unwrap_or("")
            .contains("backend unavailable"));
    }

    // --- Session log ---

    #[tokio::test]
    async fn session_log_records_every_call() {
        let engine = MockEngine {
            response: CLEAN_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let mut req_bad = clean_req("x");
        req_bad.constraints.no_network = false;
        forge.handle(&req_bad, "a").await;
        forge.handle(&clean_req("word_count"), "b").await;
        assert_eq!(forge.audit().len(), 2);
        assert!(!forge.audit()[0].accepted);
        assert!(forge.audit()[1].accepted);
    }

    // --- Memory update ---

    #[tokio::test]
    async fn memory_updated_on_success() {
        let engine = MockEngine {
            response: CLEAN_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        let report = forge.handle(&clean_req("word_count"), "input").await;
        assert!(report.memory_update.is_some());
        assert_eq!(forge.memory.len(), 1);
    }

    #[tokio::test]
    async fn memory_not_updated_on_verification_failure() {
        let engine = MockEngine {
            response: UNSAFE_RUST_RESPONSE.into(),
        };
        let soul = soul::load(None).unwrap();
        let mut forge = GenerativeForge::new(&engine, soul);
        forge.handle(&clean_req("word_count"), "input").await;
        assert_eq!(
            forge.memory.len(),
            0,
            "memory must not be updated when verification fails"
        );
    }
}
