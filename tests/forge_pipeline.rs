//! End-to-end integration tests for the full Ephemeral Tool Forge pipeline.
//!
//! Wires Phases 1–5 together using mock compiler and executor so no real
//! toolchain is needed. Validates cross-module contracts: input validation →
//! policy → reputation → code gen → static analysis → compile → execute →
//! memory update → persistence.

use async_trait::async_trait;
use split_brain_harness::{
    backends::InferenceEngine,
    capability::{Budget, CapabilityConstraints, CapabilityRequest, ToolMetrics},
    regenerative_forge::RegenerativeForge,
    reputation, soul,
    tool_memory::CapabilityMemory,
    wasm_forge::{CompileOutcome, ExecuteOutcome, WasmCompiler, WasmExecutor},
};

// ---------------------------------------------------------------------------
// Shared mock infrastructure
// ---------------------------------------------------------------------------

const MOCK_WASM: &[u8] = b"\x00asm\x01\x00\x00\x00";

fn good_rust_response() -> String {
    r#"```rust
pub fn run(input: &str) -> Result<String, String> {
    let c = input.split_whitespace().count();
    Ok(format!("{\"word_count\":{}}", c))
}
#[test] fn t1() { assert!(run("a b").is_ok()); }
#[test] fn t2() { assert!(run("").is_ok()); }
```"#
        .into()
}

fn unsafe_rust_response() -> String {
    r#"```rust
pub fn run(input: &str) -> Result<String, String> {
    unsafe { }
    Ok("ok".into())
}
#[test] fn t1() {}
#[test] fn t2() {}
```"#
        .into()
}

struct RotatingEngine {
    responses: std::sync::Mutex<std::collections::VecDeque<Result<String, String>>>,
}

impl RotatingEngine {
    fn new(responses: Vec<Result<String, String>>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl InferenceEngine for RotatingEngine {
    async fn generate(&self, _sys: &str, _prompt: &str) -> Result<String, String> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err("queue empty".into()))
    }
}

struct StaticCompiler(CompileOutcome);
impl WasmCompiler for StaticCompiler {
    fn compile(&self, _src: &str) -> CompileOutcome {
        self.0.clone()
    }
}

struct StaticExecutor(ExecuteOutcome);
impl WasmExecutor for StaticExecutor {
    fn execute(&self, _bytes: &[u8], _input: &str) -> ExecuteOutcome {
        self.0.clone()
    }
}

fn clean_req() -> CapabilityRequest {
    CapabilityRequest {
        kind: "capability_request".into(),
        capability: "word_count".into(),
        input_contract: "utf8 text".into(),
        output_contract: "json object".into(),
        constraints: CapabilityConstraints::default(),
        reason: "integration test".into(),
    }
}

// ---------------------------------------------------------------------------
// 1. Full pipeline — first attempt succeeds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_pipeline_succeeds_on_first_attempt() {
    let engine = RotatingEngine::new(vec![Ok(good_rust_response())]);
    let soul = soul::load(None).unwrap();
    let mut forge = RegenerativeForge::with_deps(
        3,
        Budget::default(),
        &engine,
        soul,
        Box::new(StaticCompiler(CompileOutcome::Success {
            wasm_bytes: MOCK_WASM.to_vec(),
            compilation_ms: 0,
        })),
        Box::new(StaticExecutor(ExecuteOutcome::Success {
            stdout: r#"{"word_count":2}"#.into(),
            execution_ms: 0,
        })),
    );

    let report = forge.handle(&clean_req(), "hello world").await;

    assert!(report.accepted, "request must be accepted");
    assert!(report.succeeded, "pipeline must succeed");
    assert_eq!(report.attempts.len(), 1, "one attempt sufficient");
    assert_eq!(report.output.as_deref(), Some(r#"{"word_count":2}"#));
    assert!(report.memory_update.is_some(), "memory must be updated");
    assert_eq!(forge.memory.len(), 1, "one pattern stored");
}

// ---------------------------------------------------------------------------
// 2. Retry on static analysis failure — feedback injected into second prompt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retries_static_failure_with_feedback() {
    let engine = RotatingEngine::new(vec![
        Ok(unsafe_rust_response()), // attempt 1 — unsafe code
        Ok(good_rust_response()),   // attempt 2 — clean
    ]);
    let soul = soul::load(None).unwrap();
    let mut forge = RegenerativeForge::with_deps(
        3,
        Budget::default(),
        &engine,
        soul,
        Box::new(StaticCompiler(CompileOutcome::Success {
            wasm_bytes: MOCK_WASM.to_vec(),
            compilation_ms: 0,
        })),
        Box::new(StaticExecutor(ExecuteOutcome::Success {
            stdout: "ok".into(),
            execution_ms: 0,
        })),
    );

    let report = forge.handle(&clean_req(), "input").await;

    assert!(report.succeeded);
    assert_eq!(report.attempts.len(), 2);

    let first = &report.attempts[0];
    assert!(!first.verification_passed);
    assert!(
        first
            .failure_reason
            .as_deref()
            .unwrap_or("")
            .contains("unsafe"),
        "failure reason must mention unsafe"
    );

    let second = &report.attempts[1];
    assert!(second.verification_passed);
    assert!(
        second.feedback_injected.is_some(),
        "second attempt must carry feedback"
    );
    assert!(
        second
            .feedback_injected
            .as_deref()
            .unwrap()
            .contains("unsafe"),
        "feedback must describe the static analysis failure"
    );
}

// ---------------------------------------------------------------------------
// 3. Blacklist: 3 prior consecutive failures → rejected before inference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blacklisted_pattern_rejected_without_inference() {
    let engine = RotatingEngine::new(vec![Ok(good_rust_response())]);
    let soul = soul::load(None).unwrap();
    let mut forge = RegenerativeForge::with_deps(
        3,
        Budget::default(),
        &engine,
        soul,
        Box::new(StaticCompiler(CompileOutcome::Success {
            wasm_bytes: MOCK_WASM.to_vec(),
            compilation_ms: 0,
        })),
        Box::new(StaticExecutor(ExecuteOutcome::Success {
            stdout: "ok".into(),
            execution_ms: 0,
        })),
    );

    // Pre-populate three consecutive failures
    let sig = CapabilityMemory::derive_signature(&clean_req());
    let rec = split_brain_harness::capability::CapabilityMemoryRecord {
        problem_signature: sig,
        solution_pattern: "regenerative:word_count".into(),
        input_shape: "utf8".into(),
        output_shape: "json".into(),
        constraints: CapabilityConstraints::default(),
    };
    let fail = ToolMetrics {
        success: false,
        ..Default::default()
    };
    forge.memory.upsert(rec.clone(), &fail);
    forge.memory.upsert(rec.clone(), &fail);
    forge.memory.upsert(rec.clone(), &fail);

    let report = forge.handle(&clean_req(), "data").await;

    assert!(!report.accepted, "blacklisted pattern must be rejected");
    assert!(
        report
            .rejection_reasons
            .iter()
            .any(|r| r.contains("blacklisted")),
        "rejection reason must say blacklisted"
    );
    assert!(
        report.attempts.is_empty(),
        "no inference budget must be spent"
    );
}

// ---------------------------------------------------------------------------
// 4. Memory persistence: save → load → reputation carries over
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_persists_and_reputation_carries_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge_memory.json");

    // --- Session 1: one successful run ---
    {
        let engine = RotatingEngine::new(vec![Ok(good_rust_response())]);
        let soul = soul::load(None).unwrap();
        let mut forge = RegenerativeForge::with_deps(
            3,
            Budget::default(),
            &engine,
            soul,
            Box::new(StaticCompiler(CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            })),
            Box::new(StaticExecutor(ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            })),
        );

        let report = forge.handle(&clean_req(), "hello world").await;
        assert!(report.succeeded);

        forge.memory.save(&path).expect("save must succeed");
    }

    // --- Session 2: load memory, check reputation improved ---
    {
        let engine = RotatingEngine::new(vec![Ok(good_rust_response())]);
        let soul = soul::load(None).unwrap();
        let mut forge = RegenerativeForge::with_deps(
            3,
            Budget::default(),
            &engine,
            soul,
            Box::new(StaticCompiler(CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            })),
            Box::new(StaticExecutor(ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            })),
        );

        let loaded = CapabilityMemory::load(&path).expect("load must succeed");
        assert_eq!(loaded.len(), 1, "one pattern must survive the round-trip");
        forge.memory = loaded;

        let report = forge.handle(&clean_req(), "hello world").await;
        assert!(report.succeeded);

        // Reputation before this session = the state loaded from file
        assert!(
            report.reputation_before.total_runs >= 1,
            "reputation_before must reflect session 1"
        );
        // After session 2 there are 2 runs, so trust should be even higher
        assert!(
            report.reputation_after.total_runs >= 2,
            "reputation_after must reflect both sessions"
        );
        assert_eq!(report.reputation_after.consecutive_failures, 0);
    }
}

// ---------------------------------------------------------------------------
// 5. Policy violation rejected before any inference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn policy_violation_rejected_cleanly() {
    let engine = RotatingEngine::new(vec![]); // no responses needed
    let soul = soul::load(None).unwrap();
    let mut forge = RegenerativeForge::with_deps(
        3,
        Budget::default(),
        &engine,
        soul,
        Box::new(StaticCompiler(CompileOutcome::CompilerNotFound {
            error: "n/a".into(),
        })),
        Box::new(StaticExecutor(ExecuteOutcome::RuntimeNotFound)),
    );

    let mut req = clean_req();
    req.constraints.no_network = false; // violates policy

    let report = forge.handle(&req, "input").await;

    assert!(!report.accepted);
    assert!(report
        .rejection_reasons
        .iter()
        .any(|r| r.contains("no_network")));
    assert!(report.attempts.is_empty());
}

// ---------------------------------------------------------------------------
// 6. Reputation scoring integration — tier progression across sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reputation_tier_improves_with_successes() {
    let engine = RotatingEngine::new(vec![
        Ok(good_rust_response()),
        Ok(good_rust_response()),
        Ok(good_rust_response()),
        Ok(good_rust_response()),
        Ok(good_rust_response()),
    ]);
    let soul = soul::load(None).unwrap();
    let mut forge = RegenerativeForge::with_deps(
        3,
        Budget {
            max_tools_per_session: 10,
            ..Budget::default()
        },
        &engine,
        soul,
        Box::new(StaticCompiler(CompileOutcome::Success {
            wasm_bytes: MOCK_WASM.to_vec(),
            compilation_ms: 0,
        })),
        Box::new(StaticExecutor(ExecuteOutcome::Success {
            stdout: "ok".into(),
            execution_ms: 0,
        })),
    );

    // Run 5 times — after 5 successes the pattern should reach Trusted tier
    for _ in 0..5 {
        forge.handle(&clean_req(), "data").await;
    }

    let sig = CapabilityMemory::derive_signature(&clean_req());
    let entry = forge.memory.lookup(&sig).expect("pattern must be stored");
    let score = reputation::compute(&entry.metrics);

    assert_eq!(entry.metrics.runs, 5);
    assert_eq!(entry.metrics.successes, 5);
    assert_eq!(
        score.tier,
        split_brain_harness::reputation::ReputationTier::Trusted
    );
}
