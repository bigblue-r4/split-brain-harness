/// Phase 5 of the Ephemeral Tool Forge — retry, reputation, and audit.
///
/// Wraps the full Phase 3+4 pipeline (generate → static-analyse → compile →
/// execute) with retry-with-feedback, Laplace-smoothed reputation scoring,
/// blacklist enforcement, and an optional append-only audit log.
///
/// On each failure, the specific failure reason is injected into the next
/// generation prompt as `<previous_failure>` context so the model can correct
/// the mistake. Patterns that accumulate BLACKLIST_THRESHOLD consecutive
/// failures are rejected immediately without spending inference budget.
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::backends::InferenceEngine;
use crate::capability::{Budget, CapabilityMemoryRecord, CapabilityRequest, ToolMetrics};
use crate::code_gen::{self, GeneratedTool};
use crate::input_validation;
use crate::policy::{self, PolicyState};
use crate::reputation::{self, ReputationScore};
use crate::tool_memory::CapabilityMemory;
use crate::types::Soul;
use crate::wasm_forge::{CompileOutcome, ExecuteOutcome, WasmCompiler, WasmExecutor};

// ---------------------------------------------------------------------------
// Per-attempt record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    /// 1-based attempt number.
    pub attempt: usize,
    /// Feedback injected from the previous attempt's failure (None on attempt 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_injected: Option<String>,
    /// Whether the LLM returned a parseable Rust code block.
    pub generation_succeeded: bool,
    /// Whether static analysis + test presence passed.
    pub verification_passed: bool,
    /// Whether rustc compiled the source to WASM.
    pub compilation_succeeded: bool,
    /// Whether wasmtime executed the WASM with exit code 0.
    pub execution_succeeded: bool,
    /// The reason this attempt failed, or None on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Full session report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegenerativeReport {
    pub accepted: bool,
    /// Non-empty when the request was rejected before any attempt.
    pub rejection_reasons: Vec<String>,
    /// All attempts made (including failed retries).
    pub attempts: Vec<AttemptRecord>,
    /// True when any attempt fully succeeded (generation → compilation → execution).
    pub succeeded: bool,
    /// Captured stdout from the successful execution, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Reputation before this session's outcome was folded in.
    pub reputation_before: ReputationScore,
    /// Reputation after this session's outcome was recorded.
    pub reputation_after: ReputationScore,
    /// Total wall-clock ms across all attempts.
    pub total_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_update: Option<CapabilityMemoryRecord>,
    /// FNV-1a-64 fingerprint of the last generated source (present when generation occurred).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

pub struct RegenerativeForge<'e> {
    /// Maximum extra attempts after the first failure (total attempts = max_retries + 1).
    pub max_retries: usize,
    budget: Budget,
    state: PolicyState,
    pub memory: CapabilityMemory,
    engine: &'e dyn InferenceEngine,
    soul: Soul,
    compiler: Box<dyn WasmCompiler>,
    executor: Box<dyn WasmExecutor>,
    session_log: Vec<RegenerativeReport>,
    /// If set, each run is appended to this JSONL file.
    pub audit_path: Option<String>,
}

impl<'e> RegenerativeForge<'e> {
    pub fn new(engine: &'e dyn InferenceEngine, soul: Soul) -> Self {
        Self::with_deps(
            3,
            Budget::default(),
            engine,
            soul,
            Box::new(crate::wasm_forge::RustcCompiler),
            Box::new(crate::wasm_forge::WasmtimeCli),
        )
    }

    pub fn with_deps(
        max_retries: usize,
        budget: Budget,
        engine: &'e dyn InferenceEngine,
        soul: Soul,
        compiler: Box<dyn WasmCompiler>,
        executor: Box<dyn WasmExecutor>,
    ) -> Self {
        Self {
            max_retries,
            budget,
            state: PolicyState::default(),
            memory: CapabilityMemory::new(),
            engine,
            soul,
            compiler,
            executor,
            session_log: vec![],
            audit_path: None,
        }
    }

    pub fn audit(&self) -> &[RegenerativeReport] {
        &self.session_log
    }

    pub async fn handle(&mut self, req: &CapabilityRequest, input: &str) -> RegenerativeReport {
        let report = self.handle_inner(req, input).await;
        if let Some(ref path) = self.audit_path {
            let entry = crate::audit::AuditEntry {
                timestamp: crate::audit::iso_now(),
                capability: req.capability.clone(),
                signature: crate::tool_memory::CapabilityMemory::derive_signature(req),
                attempt_count: report.attempts.len(),
                tier_before: format!("{:?}", report.reputation_before.tier),
                tier_after: format!("{:?}", report.reputation_after.tier),
                succeeded: report.succeeded,
                source_fingerprint: report.source_fingerprint.clone(),
                error_summary: report
                    .attempts
                    .last()
                    .and_then(|a| a.failure_reason.as_deref())
                    .map(|s| s.chars().take(200).collect()),
            };
            if let Err(e) = crate::audit::append(path, &entry) {
                eprintln!("[audit] warning: could not write to {path}: {e}");
            }
        }
        self.session_log.push(report.clone());
        report
    }

    async fn handle_inner(&mut self, req: &CapabilityRequest, input: &str) -> RegenerativeReport {
        // --- Input validation ---
        if let Err(e) = input_validation::validate_forge_input(input) {
            return pre_rejected(vec![format!("input validation: {e}")]);
        }
        if let Err(e) = input_validation::validate_capability_fields(req) {
            return pre_rejected(vec![format!("capability field validation: {e}")]);
        }

        // --- Budget check ---
        if let Some(reason) = self.state.budget_exceeded(&self.budget) {
            return pre_rejected(vec![reason]);
        }

        // --- Policy check ---
        let violations = policy::check_request(req);
        if !violations.is_empty() {
            return pre_rejected(violations.into_iter().map(|v| v.detail).collect());
        }

        // --- Reputation lookup ---
        let signature = CapabilityMemory::derive_signature(req);
        let reputation_before = match self.memory.lookup(&signature) {
            Some(entry) => reputation::compute(&entry.metrics),
            None => reputation::compute_unknown(),
        };

        if reputation_before.is_blacklisted() {
            return pre_rejected(vec![format!(
                "pattern '{}' is blacklisted after {} consecutive failures",
                signature, reputation_before.consecutive_failures
            )]);
        }

        // --- Retry loop ---
        let session_start = Instant::now();
        let mut attempts: Vec<AttemptRecord> = vec![];
        let mut feedback: Option<String> = None;
        let mut succeeded = false;
        let mut final_output: Option<String> = None;
        let mut last_source_fingerprint: Option<String> = None;

        for attempt_num in 1..=(self.max_retries + 1) {
            let prompt = match &feedback {
                None => code_gen::build_prompt(req),
                Some(fb) => build_retry_prompt(req, fb, attempt_num),
            };

            let mut record = AttemptRecord {
                attempt: attempt_num,
                feedback_injected: feedback.clone(),
                generation_succeeded: false,
                verification_passed: false,
                compilation_succeeded: false,
                execution_succeeded: false,
                failure_reason: None,
            };

            // Step 1: generate
            let raw_result = self
                .engine
                .generate(&self.soul.code_gen_system_prompt, &prompt)
                .await;

            let raw = match raw_result {
                Err(e) => {
                    let reason = format!("inference engine error: {e}");
                    record.failure_reason = Some(reason);
                    attempts.push(record);
                    break; // Engine errors are transient — don't retry
                }
                Ok(r) => r,
            };

            let source = match code_gen::extract_code_block(&raw) {
                None => {
                    // Log first 300 chars of the raw response to stderr so
                    // operators can see what the model returned instead of a
                    // code block (diagnostic aid — not sensitive data).
                    let preview: String = raw.chars().take(120).collect();
                    eprintln!(
                        "[forge] attempt {attempt_num} — no code block (raw {} chars): {preview:?}",
                        raw.len()
                    );
                    let reason = "model did not return a Rust code block".into();
                    record.failure_reason = Some(reason);
                    attempts.push(record);
                    feedback = Some(
                        "You did not return a Rust code block. \
                         You MUST respond with exactly one ```rust ... ``` block containing \
                         the full implementation."
                            .into(),
                    );
                    continue;
                }
                Some(s) => s,
            };
            record.generation_succeeded = true;
            last_source_fingerprint = Some(crate::audit::fingerprint(source.as_bytes()));

            // Step 2: static analysis + tests
            let sa = crate::static_analysis::check(&source);
            let test_count = crate::static_analysis::test_count(&source);
            let tests_included = test_count >= 2;

            if !sa.passed || !tests_included {
                let mut parts: Vec<String> = vec![];
                if !sa.passed {
                    let vlist: Vec<String> = sa
                        .violations
                        .iter()
                        .map(|v| format!("{} pattern '{}' at line {}", v.kind, v.pattern, v.line))
                        .collect();
                    parts.push(format!("Forbidden patterns found: {}", vlist.join("; ")));
                }
                if !tests_included {
                    parts.push(format!(
                        "Only {} #[test] function(s) found; at least 2 are required",
                        test_count
                    ));
                }
                let reason = parts.join(". ");
                record.failure_reason = Some(reason.clone());
                attempts.push(record);
                feedback = Some(format!(
                    "Static analysis failed: {}. \
                     Fix these issues and regenerate.",
                    reason
                ));
                continue;
            }
            record.verification_passed = true;

            // Build GeneratedTool to pass to the compiler step (used for tracking)
            let function_name =
                code_gen::extract_function_name(&source).unwrap_or_else(|| "unknown".into());
            let _tool = GeneratedTool {
                source: source.clone(),
                function_name,
                tests_included,
                test_count,
                static_analysis: sa,
            };

            // Step 3: compile
            let compile_outcome = self.compiler.compile(&source);
            let wasm_bytes = match compile_outcome {
                CompileOutcome::Success {
                    wasm_bytes,
                    compilation_ms: _,
                } => wasm_bytes,
                CompileOutcome::TargetNotInstalled { attempted_target } => {
                    let reason = format!("WASM target not installed: {attempted_target}");
                    record.failure_reason = Some(reason);
                    attempts.push(record);
                    break; // Environment issue — retrying won't help
                }
                CompileOutcome::CompilerNotFound { error } => {
                    let reason = format!("compiler not found: {error}");
                    record.failure_reason = Some(reason.clone());
                    attempts.push(record);
                    break;
                }
                CompileOutcome::CompilationFailed { stderr, .. } => {
                    let truncated: String = stderr.chars().take(512).collect();
                    let reason = format!("compilation failed: {truncated}");
                    record.failure_reason = Some(reason.clone());
                    attempts.push(record);
                    feedback = Some(format!(
                        "The Rust code did not compile. Compiler error:\n{truncated}\n\
                         Fix the syntax or type errors and regenerate."
                    ));
                    continue;
                }
            };
            record.compilation_succeeded = true;

            // Step 4: execute
            let execute_outcome = self.executor.execute(&wasm_bytes, input);
            drop(wasm_bytes);

            match execute_outcome {
                ExecuteOutcome::Success { stdout, .. } => {
                    record.execution_succeeded = true;
                    attempts.push(record);
                    succeeded = true;
                    final_output = Some(stdout);
                    break;
                }
                ExecuteOutcome::RuntimeNotFound => {
                    let reason = "wasmtime not available".into();
                    record.failure_reason = Some(reason);
                    attempts.push(record);
                    break; // Environment issue — stop
                }
                ExecuteOutcome::ExecutionFailed {
                    stderr, exit_code, ..
                } => {
                    let truncated: String = stderr.chars().take(256).collect();
                    let reason = format!("execution failed (exit {exit_code}): {truncated}");
                    record.failure_reason = Some(reason.clone());
                    attempts.push(record);
                    feedback = Some(format!(
                        "The compiled WASM exited with code {exit_code}. \
                         stderr: {truncated}\n\
                         Fix the runtime logic and regenerate."
                    ));
                    continue;
                }
                ExecuteOutcome::RuntimeError { error } => {
                    let reason = format!("runtime error: {error}");
                    record.failure_reason = Some(reason.clone());
                    attempts.push(record);
                    break;
                }
            }
        }

        let total_ms = session_start.elapsed().as_millis() as u64;

        // --- Record outcome in memory (success OR failure) ---
        let tool_metrics = ToolMetrics {
            runtime_ms: total_ms,
            input_bytes: input.len(),
            output_bytes: final_output.as_deref().map(|s| s.len()).unwrap_or(0),
            success: succeeded,
        };

        let memory_update = {
            let record = CapabilityMemoryRecord {
                problem_signature: signature.clone(),
                solution_pattern: format!("regenerative:{}", req.capability),
                input_shape: shape_token(&req.input_contract),
                output_shape: shape_token(&req.output_contract),
                constraints: req.constraints.clone(),
            };
            self.memory.upsert(record.clone(), &tool_metrics);
            self.state.record_run(&tool_metrics);
            if succeeded {
                Some(record)
            } else {
                None
            }
        };

        let reputation_after = match self.memory.lookup(&signature) {
            Some(entry) => reputation::compute(&entry.metrics),
            None => reputation::compute_unknown(),
        };

        RegenerativeReport {
            accepted: true,
            rejection_reasons: vec![],
            attempts,
            succeeded,
            output: final_output,
            reputation_before,
            reputation_after,
            total_ms,
            memory_update,
            source_fingerprint: last_source_fingerprint,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pre_rejected(reasons: Vec<String>) -> RegenerativeReport {
    RegenerativeReport {
        accepted: false,
        rejection_reasons: reasons,
        attempts: vec![],
        succeeded: false,
        output: None,
        reputation_before: reputation::compute_unknown(),
        reputation_after: reputation::compute_unknown(),
        total_ms: 0,
        memory_update: None,
        source_fingerprint: None,
    }
}

fn build_retry_prompt(req: &CapabilityRequest, failure: &str, attempt: usize) -> String {
    format!(
        "{base}\n\n<retry_context attempt=\"{attempt}\">\n\
         {failure}\n\
         </retry_context>\n\n\
         Regenerate the function. Fix the specific issue described above. \
         Do not repeat the same mistake.\n\n\
         IMPORTANT: Your response MUST contain exactly one ```rust ... ``` code block \
         and nothing else. No prose, no explanation — only the code block.",
        base = code_gen::build_prompt(req),
    )
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
    use crate::capability::CapabilityConstraints;
    use crate::wasm_forge::{CompileOutcome, ExecuteOutcome, WasmCompiler, WasmExecutor};
    use async_trait::async_trait;

    // --- Mock engine that cycles through a list of responses ---

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

    // --- Static mock compiler / executor ---

    struct MockCompiler(CompileOutcome);
    impl WasmCompiler for MockCompiler {
        fn compile(&self, _src: &str) -> CompileOutcome {
            self.0.clone()
        }
    }

    struct MockExecutor(ExecuteOutcome);
    impl WasmExecutor for MockExecutor {
        fn execute(&self, _bytes: &[u8], _input: &str) -> ExecuteOutcome {
            self.0.clone()
        }
    }

    // --- Helpers ---

    const MOCK_WASM: &[u8] = b"\x00asm\x01\x00\x00\x00";

    fn clean_req() -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: "word_count".into(),
            input_contract: "utf8 text".into(),
            output_contract: "json object".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning insufficient".into(),
        }
    }

    fn good_response() -> String {
        r#"```rust
pub fn run(input: &str) -> Result<String, String> {
    let c = input.split_whitespace().count();
    Ok(format!("{\"count\":{}}", c))
}
#[test] fn t1() { assert!(run("a b").is_ok()); }
#[test] fn t2() { assert!(run("").is_ok()); }
```"#
            .into()
    }

    fn unsafe_response() -> String {
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

    fn no_tests_response() -> String {
        r#"```rust
pub fn run(input: &str) -> Result<String, String> { Ok("ok".into()) }
```"#
            .into()
    }

    fn no_code_block_response() -> String {
        "Here is some prose with no code block.".into()
    }

    fn forge(
        responses: Vec<Result<String, String>>,
        compile: CompileOutcome,
        execute: ExecuteOutcome,
    ) -> (
        RotatingEngine,
        Soul,
        Box<dyn WasmCompiler>,
        Box<dyn WasmExecutor>,
    ) {
        let engine = RotatingEngine::new(responses);
        let soul = crate::soul::load(None).unwrap();
        let compiler: Box<dyn WasmCompiler> = Box::new(MockCompiler(compile));
        let executor: Box<dyn WasmExecutor> = Box::new(MockExecutor(execute));
        (engine, soul, compiler, executor)
    }

    // --- Acceptance path ---

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: r#"{"count":2}"#.into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "hello world").await;
        assert!(report.accepted);
        assert!(report.succeeded);
        assert_eq!(report.attempts.len(), 1);
        assert_eq!(report.attempts[0].attempt, 1);
        assert!(report.attempts[0].feedback_injected.is_none());
        assert_eq!(report.output.as_deref(), Some(r#"{"count":2}"#));
        assert!(report.memory_update.is_some());
    }

    // --- Retry paths ---

    #[tokio::test]
    async fn retries_after_static_analysis_failure() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(unsafe_response()), Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(report.succeeded);
        assert_eq!(report.attempts.len(), 2);
        // First attempt failed verification
        assert!(!report.attempts[0].verification_passed);
        assert!(report.attempts[0]
            .failure_reason
            .as_deref()
            .unwrap()
            .contains("unsafe"));
        // Second attempt succeeded
        assert!(report.attempts[1].verification_passed);
        assert!(report.attempts[1].feedback_injected.is_some());
    }

    #[tokio::test]
    async fn retries_after_missing_tests() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(no_tests_response()), Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(report.succeeded);
        assert_eq!(report.attempts.len(), 2);
        assert!(report.attempts[0]
            .failure_reason
            .as_deref()
            .unwrap()
            .contains("#[test]"));
        assert!(report.attempts[1]
            .feedback_injected
            .as_deref()
            .unwrap()
            .contains("test"));
    }

    #[tokio::test]
    async fn retries_after_no_code_block() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(no_code_block_response()), Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(report.succeeded);
        assert!(!report.attempts[0].generation_succeeded);
        assert!(report.attempts[1].generation_succeeded);
    }

    #[tokio::test]
    async fn retries_after_compilation_failure() {
        // compile returns RotatingCompiler — but our mock always returns the same thing
        // so we need a different structure here: first attempt fails compile, second succeeds
        struct RotatingCompiler(std::sync::Mutex<std::collections::VecDeque<CompileOutcome>>);
        impl WasmCompiler for RotatingCompiler {
            fn compile(&self, _src: &str) -> CompileOutcome {
                self.0
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(CompileOutcome::CompilerNotFound {
                        error: "queue empty".into(),
                    })
            }
        }

        let engine = RotatingEngine::new(vec![Ok(good_response()), Ok(good_response())]);
        let soul = crate::soul::load(None).unwrap();
        let compiler: Box<dyn WasmCompiler> = Box::new(RotatingCompiler(std::sync::Mutex::new(
            vec![
                CompileOutcome::CompilationFailed {
                    stderr: "error: mismatched types".into(),
                    compilation_ms: 0,
                },
                CompileOutcome::Success {
                    wasm_bytes: MOCK_WASM.to_vec(),
                    compilation_ms: 0,
                },
            ]
            .into(),
        )));
        let executor: Box<dyn WasmExecutor> = Box::new(MockExecutor(ExecuteOutcome::Success {
            stdout: "ok".into(),
            execution_ms: 0,
        }));
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(report.succeeded);
        assert_eq!(report.attempts.len(), 2);
        assert!(!report.attempts[0].compilation_succeeded);
        assert!(report.attempts[0]
            .failure_reason
            .as_deref()
            .unwrap()
            .contains("mismatched types"));
        assert!(report.attempts[1].compilation_succeeded);
        assert!(report.attempts[1]
            .feedback_injected
            .as_deref()
            .unwrap()
            .contains("mismatched types"));
    }

    // --- Max retries exhausted ---

    #[tokio::test]
    async fn exhausts_retries_and_fails() {
        let (engine, soul, compiler, executor) = forge(
            // 4 bad responses (max_retries=3 means 4 total attempts)
            vec![
                Ok(unsafe_response()),
                Ok(unsafe_response()),
                Ok(unsafe_response()),
                Ok(unsafe_response()),
            ],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(report.accepted);
        assert!(!report.succeeded);
        assert_eq!(report.attempts.len(), 4);
    }

    // --- Blacklist rejection ---

    #[tokio::test]
    async fn blacklisted_pattern_rejected_before_inference() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);

        // Pre-populate memory with 3 consecutive failures
        let sig = CapabilityMemory::derive_signature(&clean_req());
        let rec = crate::capability::CapabilityMemoryRecord {
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
        f.memory.upsert(rec.clone(), &fail);
        f.memory.upsert(rec.clone(), &fail);
        f.memory.upsert(rec.clone(), &fail);

        let report = f.handle(&clean_req(), "input").await;
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("blacklisted"));
        assert!(report.attempts.is_empty(), "no inference budget spent");
    }

    // --- Reputation tracks over multiple sessions ---

    #[tokio::test]
    async fn reputation_improves_after_success() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(report.succeeded);
        // After one success, trust should be higher than unknown (0.5 Laplace)
        assert!(report.reputation_after.trust > 0.5);
        assert_eq!(report.reputation_after.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn failure_updates_memory_and_increments_consecutive() {
        let (engine, soul, compiler, executor) = forge(
            // All responses fail (unsafe)
            vec![
                Ok(unsafe_response()),
                Ok(unsafe_response()),
                Ok(unsafe_response()),
                Ok(unsafe_response()),
            ],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let report = f.handle(&clean_req(), "input").await;
        assert!(!report.succeeded);
        assert!(report.reputation_after.consecutive_failures >= 1);
    }

    // --- Pre-rejection paths ---

    #[tokio::test]
    async fn rejects_oversized_input() {
        let (engine, soul, compiler, executor) = forge(
            vec![],
            CompileOutcome::CompilerNotFound { error: "x".into() },
            ExecuteOutcome::RuntimeNotFound,
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let big = "x".repeat(crate::input_validation::MAX_FORGE_INPUT_BYTES + 1);
        let report = f.handle(&clean_req(), &big).await;
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("input validation"));
    }

    #[tokio::test]
    async fn rejects_policy_violation() {
        let (engine, soul, compiler, executor) = forge(
            vec![],
            CompileOutcome::CompilerNotFound { error: "x".into() },
            ExecuteOutcome::RuntimeNotFound,
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        let mut req = clean_req();
        req.constraints.no_network = false;
        let report = f.handle(&req, "input").await;
        assert!(!report.accepted);
        assert!(report
            .rejection_reasons
            .iter()
            .any(|r| r.contains("no_network")));
    }

    #[tokio::test]
    async fn session_log_records_all_calls() {
        let (engine, soul, compiler, executor) = forge(
            vec![Ok(good_response()), Ok(good_response())],
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let mut f =
            RegenerativeForge::with_deps(3, Budget::default(), &engine, soul, compiler, executor);
        f.handle(&clean_req(), "a").await;
        f.handle(&clean_req(), "b").await;
        assert_eq!(f.audit().len(), 2);
    }
}
