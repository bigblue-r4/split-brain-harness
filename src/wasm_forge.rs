/// Phase 4 supervisor — the WASM Forge.
///
/// Takes a verified GeneratedTool (Phase 3 output), compiles it to WASM/WASI
/// using `rustc --target wasm32-wasi`, executes it in the `wasmtime` CLI
/// sandbox (no network, no filesystem), captures stdout, destroys the binary,
/// and stores fingerprint metrics only.
///
/// The supervisor code calls `std::process::Command` — that is the supervisor's
/// own privilege. Model-generated source code is still forbidden from using it
/// (enforced by static analysis in Phase 3).
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::capability::{Budget, CapabilityMemoryRecord, CapabilityRequest, ToolMetrics};
use crate::code_gen::GeneratedTool;
use crate::input_validation;
use crate::policy::{self, PolicyState};
use crate::tool_memory::CapabilityMemory;

// ---------------------------------------------------------------------------
// WASM main() wrapper
//
// Appended to the generated source before compilation. Reads from stdin,
// calls `run()`, writes JSON to stdout.
// ---------------------------------------------------------------------------

const WASM_MAIN: &str = r#"

fn main() {
    use std::io::Read;
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        std::process::exit(2);
    }
    match run(&input) {
        Ok(output) => print!("{}", output),
        Err(e) => {
            eprintln!("run error: {}", e);
            std::process::exit(1);
        }
    }
}
"#;

// ---------------------------------------------------------------------------
// Outcome enums
// ---------------------------------------------------------------------------

/// Result of the `rustc --target wasm32-wasi` compilation step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompileOutcome {
    Success {
        wasm_bytes: Vec<u8>,
        compilation_ms: u64,
    },
    /// The wasm32-wasi (or wasm32-wasip1) target is not installed.
    TargetNotInstalled { attempted_target: String },
    /// rustc ran but returned a non-zero exit code.
    CompilationFailed { stderr: String, compilation_ms: u64 },
    /// rustc could not be spawned (not on PATH, OS error, etc.).
    CompilerNotFound { error: String },
}

/// Result of executing the compiled WASM binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecuteOutcome {
    Success {
        stdout: String,
        execution_ms: u64,
    },
    /// wasmtime binary is not available on PATH.
    RuntimeNotFound,
    /// wasmtime ran but the WASM process exited non-zero.
    ExecutionFailed {
        stderr: String,
        exit_code: i32,
        execution_ms: u64,
    },
    /// wasmtime could not be spawned (OS error).
    RuntimeError {
        error: String,
    },
}

// ---------------------------------------------------------------------------
// Phase 4 report
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WasmMetrics {
    pub compilation_ms: u64,
    pub execution_ms: u64,
    pub wasm_binary_bytes: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WasmExecutionReport {
    pub accepted: bool,
    pub rejection_reasons: Vec<String>,
    /// True when compilation succeeded.
    pub compiled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compilation_error: Option<String>,
    /// True when the WASM executed successfully (exit code 0).
    pub executed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_error: Option<String>,
    /// Captured stdout from the WASM process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Binary destroyed after use: always true once compilation completes.
    pub destroyed: bool,
    pub metrics: WasmMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_update: Option<CapabilityMemoryRecord>,
}

// ---------------------------------------------------------------------------
// Compiler and executor traits — injectable for testing
// ---------------------------------------------------------------------------

pub trait WasmCompiler: Send + Sync {
    fn compile(&self, source: &str) -> CompileOutcome;
}

pub trait WasmExecutor: Send + Sync {
    fn execute(&self, wasm_bytes: &[u8], input: &str) -> ExecuteOutcome;
}

// ---------------------------------------------------------------------------
// Real compiler: rustc --target wasm32-wasi
// ---------------------------------------------------------------------------

pub struct RustcCompiler;

impl WasmCompiler for RustcCompiler {
    fn compile(&self, source: &str) -> CompileOutcome {
        // Detect installed WASM target (wasm32-wasip1 is the new name, wasm32-wasi the old)
        let target = match detect_wasm_target() {
            Some(t) => t,
            None => {
                return CompileOutcome::TargetNotInstalled {
                    attempted_target: "wasm32-wasip1 / wasm32-wasi".into(),
                }
            }
        };

        // Write source + wrapper to a temp file
        let tmp_dir = std::env::temp_dir().join(format!("sbh-wasm-{}", monotonic_id()));
        if std::fs::create_dir_all(&tmp_dir).is_err() {
            return CompileOutcome::CompilationFailed {
                stderr: "failed to create temp directory".into(),
                compilation_ms: 0,
            };
        }

        let src_path = tmp_dir.join("tool.rs");
        let wasm_path = tmp_dir.join("tool.wasm");
        let full_source = format!("{}\n{}", source, WASM_MAIN);

        if std::fs::write(&src_path, &full_source).is_err() {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return CompileOutcome::CompilationFailed {
                stderr: "failed to write source file".into(),
                compilation_ms: 0,
            };
        }

        let start = Instant::now();
        let result = Command::new("rustc")
            .args(["--target", target, "--edition", "2021", "-o"])
            .arg(&wasm_path)
            .arg(&src_path)
            .output();
        let compilation_ms = start.elapsed().as_millis() as u64;

        // Remove source immediately (never persist model-generated code)
        let _ = std::fs::remove_file(&src_path);

        match result {
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                CompileOutcome::CompilerNotFound {
                    error: format!("could not spawn rustc: {e}"),
                }
            }
            Ok(out) if !out.status.success() => {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                CompileOutcome::CompilationFailed {
                    stderr: String::from_utf8_lossy(&out.stderr)
                        .chars()
                        .take(2048)
                        .collect(),
                    compilation_ms,
                }
            }
            Ok(_) => {
                let wasm_bytes = std::fs::read(&wasm_path).unwrap_or_default();
                // Destroy the compiled WASM — metrics only survive
                let _ = std::fs::remove_dir_all(&tmp_dir);
                CompileOutcome::Success {
                    wasm_bytes,
                    compilation_ms,
                }
            }
        }
    }
}

/// Detect an installed WASM/WASI target via `rustup target list --installed`.
/// Returns the preferred target name, or None if neither is installed.
fn detect_wasm_target() -> Option<&'static str> {
    let out = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()?;
    let targets = String::from_utf8_lossy(&out.stdout);
    if targets.contains("wasm32-wasip1") {
        Some("wasm32-wasip1")
    } else if targets.contains("wasm32-wasi") {
        Some("wasm32-wasi")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Real executor: wasmtime CLI subprocess
// ---------------------------------------------------------------------------

pub struct WasmtimeCli;

impl WasmExecutor for WasmtimeCli {
    fn execute(&self, wasm_bytes: &[u8], input: &str) -> ExecuteOutcome {
        // Write WASM to a temp file — destroyed immediately after the call
        let tmp_wasm = std::env::temp_dir().join(format!("sbh-run-{}.wasm", monotonic_id()));
        if std::fs::write(&tmp_wasm, wasm_bytes).is_err() {
            return ExecuteOutcome::RuntimeError {
                error: "failed to write wasm to temp file".into(),
            };
        }

        let start = Instant::now();

        // Spawn wasmtime and feed input via stdin. We wait for the process to
        // finish BEFORE deleting the WASM file — spawning is non-blocking so
        // deleting first would cause a race where wasmtime tries to open a
        // file that no longer exists.
        let spawn_result = Command::new("wasmtime")
            .arg("--")
            .arg(&tmp_wasm)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let output_result = match spawn_result {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(&tmp_wasm);
                return ExecuteOutcome::RuntimeNotFound;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_wasm);
                return ExecuteOutcome::RuntimeError {
                    error: format!("spawn error: {e}"),
                };
            }
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(input.as_bytes());
                }
                child.wait_with_output()
            }
        };

        // WASM binary destroyed after the process has exited and closed the fd
        let _ = std::fs::remove_file(&tmp_wasm);

        match output_result {
            Err(e) => ExecuteOutcome::RuntimeError {
                error: format!("wait error: {e}"),
            },
            Ok(out) => {
                let execution_ms = start.elapsed().as_millis() as u64;
                let stdout = String::from_utf8_lossy(&out.stdout)
                    .chars()
                    .take(65_536)
                    .collect();
                let exit_code = out.status.code().unwrap_or(-1);

                if out.status.success() {
                    ExecuteOutcome::Success {
                        stdout,
                        execution_ms,
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr)
                        .chars()
                        .take(1024)
                        .collect();
                    ExecuteOutcome::ExecutionFailed {
                        stderr,
                        exit_code,
                        execution_ms,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

pub struct WasmForge {
    budget: Budget,
    state: PolicyState,
    pub memory: CapabilityMemory,
    compiler: Box<dyn WasmCompiler>,
    executor: Box<dyn WasmExecutor>,
    session_log: Vec<WasmExecutionReport>,
}

impl WasmForge {
    pub fn new() -> Self {
        Self::with_deps(Box::new(RustcCompiler), Box::new(WasmtimeCli))
    }

    pub fn with_deps(compiler: Box<dyn WasmCompiler>, executor: Box<dyn WasmExecutor>) -> Self {
        Self {
            // Tighter budget for Phase 4: compilation is expensive
            budget: Budget {
                max_tools_per_session: 2,
                max_total_runtime_ms: 120_000, // 2 minutes total
                require_approval_after_failures: 1,
            },
            state: PolicyState::default(),
            memory: CapabilityMemory::new(),
            compiler,
            executor,
            session_log: vec![],
        }
    }

    pub fn audit(&self) -> &[WasmExecutionReport] {
        &self.session_log
    }

    /// Process a Phase 3 GeneratedTool:
    ///   validate → policy check → verify tool passed Phase 3 →
    ///   compile → execute → destroy → memory update
    pub fn handle(
        &mut self,
        req: &CapabilityRequest,
        tool: &GeneratedTool,
        input: &str,
    ) -> WasmExecutionReport {
        let report = self.handle_inner(req, tool, input);
        self.session_log.push(report.clone());
        report
    }

    fn handle_inner(
        &mut self,
        req: &CapabilityRequest,
        tool: &GeneratedTool,
        input: &str,
    ) -> WasmExecutionReport {
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

        // Require Phase 3 verification to have passed
        if !tool.static_analysis.passed {
            let reasons: Vec<String> = tool
                .static_analysis
                .violations
                .iter()
                .map(|v| format!("static_analysis: {} at line {}", v.kind, v.line))
                .collect();
            return rejected(reasons);
        }
        if !tool.tests_included {
            return rejected(vec![
                "generated tool does not include the required minimum of 2 #[test] functions"
                    .into(),
            ]);
        }

        // Compile
        let compile_outcome = self.compiler.compile(&tool.source);
        let (wasm_bytes, compilation_ms) = match compile_outcome {
            CompileOutcome::Success {
                wasm_bytes,
                compilation_ms,
            } => (wasm_bytes, compilation_ms),
            CompileOutcome::TargetNotInstalled { attempted_target } => {
                let metrics = zero_metrics(input);
                self.state.record_run(&ToolMetrics {
                    success: false,
                    input_bytes: input.len(),
                    ..Default::default()
                });
                return WasmExecutionReport {
                    accepted: true,
                    rejection_reasons: vec![],
                    compiled: false,
                    compilation_error: Some(format!(
                        "WASM target not installed: {attempted_target} — \
                         run: rustup target add wasm32-wasip1"
                    )),
                    executed: false,
                    execution_error: None,
                    output: None,
                    destroyed: true,
                    metrics,
                    memory_update: None,
                };
            }
            CompileOutcome::CompilationFailed {
                stderr,
                compilation_ms,
            } => {
                let metrics = WasmMetrics {
                    compilation_ms,
                    ..zero_metrics(input)
                };
                self.state.record_run(&ToolMetrics {
                    success: false,
                    input_bytes: input.len(),
                    runtime_ms: compilation_ms,
                    ..Default::default()
                });
                return WasmExecutionReport {
                    accepted: true,
                    rejection_reasons: vec![],
                    compiled: false,
                    compilation_error: Some(stderr),
                    executed: false,
                    execution_error: None,
                    output: None,
                    destroyed: true,
                    metrics,
                    memory_update: None,
                };
            }
            CompileOutcome::CompilerNotFound { error } => {
                let metrics = zero_metrics(input);
                self.state.record_run(&ToolMetrics {
                    success: false,
                    input_bytes: input.len(),
                    ..Default::default()
                });
                return WasmExecutionReport {
                    accepted: true,
                    rejection_reasons: vec![],
                    compiled: false,
                    compilation_error: Some(error),
                    executed: false,
                    execution_error: None,
                    output: None,
                    destroyed: true,
                    metrics,
                    memory_update: None,
                };
            }
        };

        // WASM binary size (before destruction by executor)
        let wasm_binary_bytes = wasm_bytes.len();

        // Execute
        let execute_outcome = self.executor.execute(&wasm_bytes, input);
        // wasm_bytes is no longer needed — drop it now
        drop(wasm_bytes);

        let (executed, stdout, execution_ms, execution_error) = match execute_outcome {
            ExecuteOutcome::Success {
                stdout,
                execution_ms,
            } => (true, Some(stdout), execution_ms, None),
            ExecuteOutcome::RuntimeNotFound => (
                false,
                None,
                0,
                Some("wasmtime not found on PATH — install from https://wasmtime.dev".into()),
            ),
            ExecuteOutcome::ExecutionFailed {
                stderr,
                execution_ms,
                ..
            } => (false, None, execution_ms, Some(stderr)),
            ExecuteOutcome::RuntimeError { error } => (false, None, 0, Some(error)),
        };

        let success = executed && execution_error.is_none();
        let tool_metrics = ToolMetrics {
            runtime_ms: compilation_ms + execution_ms,
            input_bytes: input.len(),
            output_bytes: stdout.as_deref().map(|s| s.len()).unwrap_or(0),
            success,
        };
        self.state.record_run(&tool_metrics);

        let metrics = WasmMetrics {
            compilation_ms,
            execution_ms,
            wasm_binary_bytes,
            input_bytes: input.len(),
            output_bytes: stdout.as_deref().map(|s| s.len()).unwrap_or(0),
        };

        // Update capability memory on success
        let memory_update = if success {
            let signature = CapabilityMemory::derive_signature(req);
            let record = CapabilityMemoryRecord {
                problem_signature: signature,
                solution_pattern: format!("wasm:{}", req.capability),
                input_shape: shape_token(&req.input_contract),
                output_shape: shape_token(&req.output_contract),
                constraints: req.constraints.clone(),
            };
            self.memory.upsert(record.clone(), &tool_metrics);
            Some(record)
        } else {
            None
        };

        WasmExecutionReport {
            accepted: true,
            rejection_reasons: vec![],
            compiled: true,
            compilation_error: None,
            executed,
            execution_error,
            output: stdout,
            destroyed: true,
            metrics,
            memory_update,
        }
    }

    pub fn tools_invoked(&self) -> usize {
        self.state.tools_invoked
    }
}

impl Default for WasmForge {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rejected(reasons: Vec<String>) -> WasmExecutionReport {
    WasmExecutionReport {
        accepted: false,
        rejection_reasons: reasons,
        compiled: false,
        compilation_error: None,
        executed: false,
        execution_error: None,
        output: None,
        destroyed: false,
        metrics: WasmMetrics {
            compilation_ms: 0,
            execution_ms: 0,
            wasm_binary_bytes: 0,
            input_bytes: 0,
            output_bytes: 0,
        },
        memory_update: None,
    }
}

fn zero_metrics(input: &str) -> WasmMetrics {
    WasmMetrics {
        compilation_ms: 0,
        execution_ms: 0,
        wasm_binary_bytes: 0,
        input_bytes: input.len(),
        output_bytes: 0,
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

fn monotonic_id() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}{}", d.as_secs(), d.subsec_nanos()))
        .unwrap_or_else(|_| "0".into())
}

// ---------------------------------------------------------------------------
// Tests — use mock compiler + executor so CI works without wasm32-wasi
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityConstraints;
    use crate::static_analysis;

    // --- Mock compiler ---

    struct MockCompiler(CompileOutcome);

    impl WasmCompiler for MockCompiler {
        fn compile(&self, _source: &str) -> CompileOutcome {
            self.0.clone()
        }
    }

    // --- Mock executor ---

    struct MockExecutor(ExecuteOutcome);

    impl WasmExecutor for MockExecutor {
        fn execute(&self, _bytes: &[u8], _input: &str) -> ExecuteOutcome {
            self.0.clone()
        }
    }

    // --- Test helpers ---

    fn clean_req(cap: &str) -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: cap.into(),
            input_contract: "utf8 text".into(),
            output_contract: "json object".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning insufficient".into(),
        }
    }

    fn verified_tool() -> GeneratedTool {
        let source = r#"pub fn run(input: &str) -> Result<String, String> {
    let c = input.split_whitespace().count();
    Ok(format!("{\"count\":{}}", c))
}
#[test] fn t1() { assert!(run("a b").is_ok()); }
#[test] fn t2() { assert!(run("").is_ok()); }"#;
        GeneratedTool {
            source: source.into(),
            function_name: "run".into(),
            tests_included: true,
            test_count: 2,
            static_analysis: static_analysis::check(source),
        }
    }

    fn unverified_tool_unsafe() -> GeneratedTool {
        let source = r#"pub fn run(input: &str) -> Result<String, String> {
    unsafe { }
    Ok("ok".into())
}
#[test] fn t1() {}
#[test] fn t2() {}"#;
        GeneratedTool {
            source: source.into(),
            function_name: "run".into(),
            tests_included: true,
            test_count: 2,
            static_analysis: static_analysis::check(source),
        }
    }

    fn unverified_tool_no_tests() -> GeneratedTool {
        let source = "pub fn run(input: &str) -> Result<String, String> { Ok(\"ok\".into()) }";
        GeneratedTool {
            source: source.into(),
            function_name: "run".into(),
            tests_included: false,
            test_count: 0,
            static_analysis: static_analysis::check(source),
        }
    }

    fn forge_with(compile: CompileOutcome, exec: ExecuteOutcome) -> WasmForge {
        WasmForge::with_deps(
            Box::new(MockCompiler(compile)),
            Box::new(MockExecutor(exec)),
        )
    }

    const MOCK_WASM: &[u8] = b"\x00asm\x01\x00\x00\x00"; // minimal WASM magic bytes

    // --- Acceptance path ---

    #[test]
    fn successful_compile_and_execute() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 800,
            },
            ExecuteOutcome::Success {
                stdout: r#"{"count":2}"#.into(),
                execution_ms: 12,
            },
        );
        let report = forge.handle(&clean_req("word_count"), &verified_tool(), "hello world");
        assert!(report.accepted);
        assert!(report.compiled);
        assert!(report.executed);
        assert!(report.destroyed, "binary must be destroyed after execution");
        assert_eq!(report.output.as_deref(), Some(r#"{"count":2}"#));
        assert!(report.memory_update.is_some());
    }

    // --- Rejection before compilation ---

    #[test]
    fn rejects_tool_that_failed_static_analysis() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let report = forge.handle(&clean_req("x"), &unverified_tool_unsafe(), "input");
        assert!(!report.accepted);
        assert!(report
            .rejection_reasons
            .iter()
            .any(|r| r.contains("static_analysis")));
    }

    #[test]
    fn rejects_tool_without_tests() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let report = forge.handle(&clean_req("x"), &unverified_tool_no_tests(), "input");
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("#[test]"));
    }

    #[test]
    fn rejects_policy_violation() {
        let mut req = clean_req("fetch_url");
        req.constraints.no_network = false;
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::RuntimeNotFound,
        );
        let report = forge.handle(&req, &verified_tool(), "input");
        assert!(!report.accepted);
        assert!(report
            .rejection_reasons
            .iter()
            .any(|r| r.contains("no_network")));
    }

    #[test]
    fn rejects_oversized_input() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        let big = "x".repeat(crate::input_validation::MAX_FORGE_INPUT_BYTES + 1);
        let report = forge.handle(&clean_req("x"), &verified_tool(), &big);
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("input validation"));
    }

    #[test]
    fn rejects_when_budget_exhausted() {
        let mut forge = WasmForge {
            budget: Budget {
                max_tools_per_session: 1,
                ..Budget::default()
            },
            state: PolicyState::default(),
            memory: CapabilityMemory::new(),
            compiler: Box::new(MockCompiler(CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            })),
            executor: Box::new(MockExecutor(ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            })),
            session_log: vec![],
        };
        forge.handle(&clean_req("x"), &verified_tool(), "a");
        let report = forge.handle(&clean_req("x"), &verified_tool(), "b");
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("session tool limit"));
    }

    // --- Compilation failure paths ---

    #[test]
    fn compilation_failure_reports_stderr() {
        let mut forge = forge_with(
            CompileOutcome::CompilationFailed {
                stderr: "error[E0001]: syntax error".into(),
                compilation_ms: 300,
            },
            ExecuteOutcome::RuntimeNotFound,
        );
        let report = forge.handle(&clean_req("x"), &verified_tool(), "input");
        assert!(report.accepted);
        assert!(!report.compiled);
        assert!(!report.executed);
        assert!(report.destroyed, "nothing to destroy but flag must be set");
        assert!(report
            .compilation_error
            .as_deref()
            .unwrap_or("")
            .contains("syntax error"));
    }

    #[test]
    fn target_not_installed_returns_clear_message() {
        let mut forge = forge_with(
            CompileOutcome::TargetNotInstalled {
                attempted_target: "wasm32-wasip1".into(),
            },
            ExecuteOutcome::RuntimeNotFound,
        );
        let report = forge.handle(&clean_req("x"), &verified_tool(), "input");
        assert!(report.accepted);
        assert!(!report.compiled);
        let err = report.compilation_error.unwrap_or_default();
        assert!(err.contains("wasm32-wasip1") || err.contains("target not installed"));
    }

    // --- Execution failure paths ---

    #[test]
    fn runtime_not_found_does_not_panic() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 500,
            },
            ExecuteOutcome::RuntimeNotFound,
        );
        let report = forge.handle(&clean_req("x"), &verified_tool(), "input");
        assert!(report.accepted);
        assert!(report.compiled);
        assert!(!report.executed);
        assert!(report
            .execution_error
            .as_deref()
            .unwrap_or("")
            .contains("wasmtime"));
    }

    #[test]
    fn execution_failure_reports_stderr() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 500,
            },
            ExecuteOutcome::ExecutionFailed {
                stderr: "runtime trap".into(),
                exit_code: 1,
                execution_ms: 5,
            },
        );
        let report = forge.handle(&clean_req("x"), &verified_tool(), "input");
        assert!(report.compiled);
        assert!(!report.executed);
        assert!(report
            .execution_error
            .as_deref()
            .unwrap_or("")
            .contains("runtime trap"));
    }

    // --- Memory and audit ---

    #[test]
    fn memory_not_updated_when_execution_fails() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::RuntimeNotFound,
        );
        forge.handle(&clean_req("x"), &verified_tool(), "input");
        assert_eq!(forge.memory.len(), 0);
    }

    #[test]
    fn session_log_records_all_calls() {
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 0,
            },
            ExecuteOutcome::Success {
                stdout: "ok".into(),
                execution_ms: 0,
            },
        );
        forge.handle(&clean_req("x"), &verified_tool(), "a");
        let mut req2 = clean_req("y");
        req2.constraints.no_network = false;
        forge.handle(&req2, &verified_tool(), "b");
        assert_eq!(forge.audit().len(), 2);
    }

    #[test]
    fn wasm_wrapper_contains_stdin_read_and_run_call() {
        assert!(WASM_MAIN.contains("read_to_string"));
        assert!(WASM_MAIN.contains("run(&input)"));
        assert!(WASM_MAIN.contains("process::exit"));
    }

    #[test]
    fn metrics_record_binary_bytes() {
        let wasm = vec![0u8; 42_000];
        let mut forge = forge_with(
            CompileOutcome::Success {
                wasm_bytes: wasm,
                compilation_ms: 900,
            },
            ExecuteOutcome::Success {
                stdout: "result".into(),
                execution_ms: 15,
            },
        );
        let report = forge.handle(&clean_req("x"), &verified_tool(), "input");
        assert_eq!(report.metrics.wasm_binary_bytes, 42_000);
        assert_eq!(report.metrics.compilation_ms, 900);
        assert_eq!(report.metrics.execution_ms, 15);
    }
}
