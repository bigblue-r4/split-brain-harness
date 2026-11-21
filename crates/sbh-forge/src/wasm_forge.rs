//! Phase 4 of the Ephemeral Tool Forge — WASM/WASI sandboxed execution.
//!
//! Takes a verified `GeneratedTool` (Phase 3 output), compiles it to
//! `wasm32-wasip1`, executes it in the `wasmtime` CLI sandbox (no network,
//! no filesystem access), captures stdout, then destroys the binary.
//!
//! Requires Linux, bubblewrap, prlimit, a WASI Rust target and wasmtime.
//! Compilation fails closed when the isolation boundary is unavailable.
//! Static analysis is defense in depth, not a compiler-host sandbox.
use crate::process_boundary::{self, Workspace};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const COMPILE_TIMEOUT: Duration = Duration::from_secs(60);
const WASM_EXEC_TIMEOUT_MS: u64 = 10_000;
const MAX_OUTPUT: usize = 65_536;

use serde::{Deserialize, Serialize};

use crate::code_gen::GeneratedTool;
use crate::tool_memory::CapabilityMemory;
use sbh_core::capability::{Budget, CapabilityMemoryRecord, CapabilityRequest, ToolMetrics};
use sbh_core::input_validation;
use sbh_safety::policy::{self, PolicyState};

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
    /// Temporary artifacts could not be removed; never report destroyed.
    CleanupFailed { error: String },
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
    /// Temporary artifacts could not be removed; never report destroyed.
    CleanupFailed {
        error: String,
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
    /// On-disk temporary artifacts were removed (not a secure-erasure guarantee).
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
    /// Custom implementations must honor limits; the real executor enforces them.
    fn execute_limited(
        &self,
        wasm_bytes: &[u8],
        input: &str,
        _runtime_ms: u64,
        _memory_mb: u64,
    ) -> ExecuteOutcome {
        self.execute(wasm_bytes, input)
    }
}

// ---------------------------------------------------------------------------
// Real compiler: rustc --target wasm32-wasi
// ---------------------------------------------------------------------------

pub struct RustcCompiler;

impl WasmCompiler for RustcCompiler {
    fn compile(&self, source: &str) -> CompileOutcome {
        if source.len() > process_boundary::MAX_INPUT {
            return compile_error("source limit exceeded", 0);
        }
        let workspace = match Workspace::new() {
            Ok(w) => w,
            Err(e) => return compile_error(&format!("isolation unavailable: {e}"), 0),
        };
        let result = compile_isolated(source, &workspace);
        if let Err(e) = workspace.cleanup() {
            return CompileOutcome::CleanupFailed {
                error: format!("compiler workspace cleanup failed: {e}"),
            };
        }
        result
    }
}

fn compile_error(error: &str, compilation_ms: u64) -> CompileOutcome {
    CompileOutcome::CompilationFailed {
        stderr: error.into(),
        compilation_ms,
    }
}

fn compile_isolated(source: &str, workspace: &Workspace) -> CompileOutcome {
    let start = Instant::now();
    // This query consumes no generated source. Bound its output and lifetime too.
    let mut query = Command::new("rustc");
    query.args(["--print", "sysroot"]);
    let result = match process_boundary::run(&mut query, b"", Duration::from_secs(5), 4096) {
        Ok(out) if out.status.success() => out,
        _ => {
            return CompileOutcome::CompilerNotFound {
                error: "could not locate Rust sysroot".into(),
            }
        }
    };
    let sysroot = match PathBuf::from(String::from_utf8_lossy(&result.stdout).trim()).canonicalize()
    {
        Ok(root) => root,
        Err(e) => return compile_error(&format!("invalid Rust sysroot: {e}"), 0),
    };
    let targets = ["wasm32-wasip1", "wasm32-wasi"];
    let target = match targets
        .iter()
        .find(|t| sysroot.join("lib/rustlib").join(t).join("lib").is_dir())
    {
        Some(t) => *t,
        None => {
            return CompileOutcome::TargetNotInstalled {
                attempted_target: targets.join(" / "),
            }
        }
    };
    if let Err(e) = workspace.write("tool.rs", format!("{source}\n{WASM_MAIN}").as_bytes()) {
        return compile_error(&format!("source write failed: {e}"), 0);
    }
    let mut command = match process_boundary::compiler_command(&sysroot, &workspace.path) {
        Ok(c) => c,
        Err(e) => return compile_error(&format!("compiler isolation unavailable: {e}"), 0),
    };
    command.args([
        "--target",
        target,
        "--edition",
        "2021",
        "-o",
        "/work/tool.wasm",
        "/work/tool.rs",
    ]);
    let result = process_boundary::run(&mut command, b"", COMPILE_TIMEOUT, 8192);
    let elapsed = start.elapsed().as_millis() as u64;
    match result {
        Err(e) => compile_error(&format!("isolated compilation failed: {e}"), elapsed),
        Ok(out) if !out.status.success() => {
            compile_error(&String::from_utf8_lossy(&out.stderr), elapsed)
        }
        Ok(_) => {
            let artifact = workspace.path.join("tool.wasm");
            match std::fs::metadata(&artifact).and_then(|meta| {
                if !meta.is_file() || meta.len() == 0 || meta.len() > process_boundary::MAX_ARTIFACT
                {
                    return Err(std::io::Error::other("invalid compiled artifact size"));
                }
                std::fs::read(&artifact)
            }) {
                Ok(wasm_bytes) => CompileOutcome::Success {
                    wasm_bytes,
                    compilation_ms: elapsed,
                },
                Err(e) => compile_error(&format!("artifact read failed: {e}"), elapsed),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Real executor: bounded wasmtime CLI subprocess (no preopens / inherited env)
// ---------------------------------------------------------------------------

// Fixed startup/compilation allowance; guest execution still has runtime_ms.
// The whole-call I/O deadline and CPU limit remain finite even before guest entry.
const RUNTIME_STARTUP_SLACK_MS: u64 = 1_000;
fn runtime_deadline(runtime_ms: u64) -> Duration {
    Duration::from_millis(runtime_ms.saturating_add(RUNTIME_STARTUP_SLACK_MS))
}

pub struct WasmtimeCli;
impl WasmExecutor for WasmtimeCli {
    fn execute(&self, wasm_bytes: &[u8], input: &str) -> ExecuteOutcome {
        self.execute_limited(wasm_bytes, input, WASM_EXEC_TIMEOUT_MS, 64)
    }
    fn execute_limited(
        &self,
        wasm_bytes: &[u8],
        input: &str,
        runtime_ms: u64,
        memory_mb: u64,
    ) -> ExecuteOutcome {
        if input.len() > process_boundary::MAX_INPUT
            || wasm_bytes.len() as u64 > process_boundary::MAX_ARTIFACT
            || runtime_ms == 0
            || runtime_ms > WASM_EXEC_TIMEOUT_MS
            || memory_mb == 0
            || memory_mb > 256
        {
            return ExecuteOutcome::RuntimeError {
                error: "executor input/resource limit exceeded".into(),
            };
        }
        let workspace = match Workspace::new() {
            Ok(w) => w,
            Err(e) => {
                return ExecuteOutcome::RuntimeError {
                    error: format!("runtime workspace failed: {e}"),
                }
            }
        };
        let result = execute_isolated(wasm_bytes, input, runtime_ms, memory_mb, &workspace);
        if let Err(e) = workspace.cleanup() {
            return ExecuteOutcome::CleanupFailed {
                error: format!("runtime workspace cleanup failed: {e}"),
            };
        }
        result
    }
}

fn execute_isolated(
    wasm_bytes: &[u8],
    input: &str,
    runtime_ms: u64,
    memory_mb: u64,
    workspace: &Workspace,
) -> ExecuteOutcome {
    if let Err(e) = workspace.write("tool.wasm", wasm_bytes) {
        return ExecuteOutcome::RuntimeError {
            error: format!("runtime artifact write failed: {e}"),
        };
    }
    // Resolve the operator-installed executable before clearing the environment.
    // The WASI guest gets no filesystem preopens, sockets or environment options.
    let runtime = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("wasmtime"))
        .find(|p| p.is_file());
    let runtime = match runtime.and_then(|p| p.canonicalize().ok()) {
        Some(p) => p,
        None => return ExecuteOutcome::RuntimeNotFound,
    };
    // The runtime executable itself runs inside the same namespace boundary as rustc.
    let mut command = match process_boundary::runtime_command(
        &runtime,
        &workspace.path,
        runtime_ms + RUNTIME_STARTUP_SLACK_MS,
    ) {
        Ok(command) => command,
        Err(e) => {
            return ExecuteOutcome::RuntimeError {
                error: format!("runtime isolation unavailable: {e}"),
            }
        }
    };
    command
        .args([
            "-C",
            "cache=n",
            "-C",
            "parallel-compilation=n",
            "-W",
            &format!("timeout={runtime_ms}ms"),
            "-W",
            &format!("max-memory-size={}", memory_mb * 1_048_576),
            "-W",
            "max-memories=1",
            "-W",
            "max-tables=1",
            "-W",
            "max-table-elements=10000",
            "--",
        ])
        .arg("/work/tool.wasm");
    let start = Instant::now();
    match process_boundary::run(
        &mut command,
        input.as_bytes(),
        runtime_deadline(runtime_ms),
        MAX_OUTPUT,
    ) {
        Err(e) => ExecuteOutcome::RuntimeError {
            error: format!("bounded runtime failed: {e}"),
        },
        Ok(out) if out.status.success() => ExecuteOutcome::Success {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            execution_ms: start.elapsed().as_millis() as u64,
        },
        Ok(out) => ExecuteOutcome::ExecutionFailed {
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
            execution_ms: start.elapsed().as_millis() as u64,
        },
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
            CompileOutcome::CleanupFailed { error } => {
                self.state.record_run(&ToolMetrics {
                    success: false,
                    input_bytes: input.len(),
                    ..Default::default()
                });
                return WasmExecutionReport {
                    accepted: true,
                    compiled: false,
                    compilation_error: Some(error),
                    destroyed: false,
                    ..rejected(vec![])
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
        let execution_started = Instant::now();
        let execute_outcome = self.executor.execute_limited(
            &wasm_bytes,
            input,
            req.constraints.max_runtime_ms,
            req.constraints.max_memory_mb,
        );
        // wasm_bytes is no longer needed — drop it now
        drop(wasm_bytes);

        let failed_execution_ms = execution_started.elapsed().as_millis() as u64;
        let destroyed = !matches!(&execute_outcome, ExecuteOutcome::CleanupFailed { .. });
        let (executed, stdout, execution_ms, execution_error) = match execute_outcome {
            ExecuteOutcome::Success {
                stdout,
                execution_ms,
            } => (true, Some(stdout), execution_ms, None),
            ExecuteOutcome::CleanupFailed { error } => {
                (false, None, failed_execution_ms, Some(error))
            }
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
            ExecuteOutcome::RuntimeError { error } => {
                (false, None, failed_execution_ms, Some(error))
            }
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
            destroyed,
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

// ---------------------------------------------------------------------------
// Tests — use mock compiler + executor so CI works without wasm32-wasi
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_analysis;
    use sbh_core::capability::CapabilityConstraints;

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

    #[test]
    fn cleanup_failures_never_claim_destruction_or_admit_memory() {
        for (compile, execute) in [
            (
                CompileOutcome::CleanupFailed {
                    error: "synthetic cleanup failure".into(),
                },
                ExecuteOutcome::RuntimeNotFound,
            ),
            (
                CompileOutcome::Success {
                    wasm_bytes: MOCK_WASM.to_vec(),
                    compilation_ms: 1,
                },
                ExecuteOutcome::CleanupFailed {
                    error: "synthetic cleanup failure".into(),
                },
            ),
        ] {
            let mut forge = forge_with(compile, execute);
            let report = forge.handle(&clean_req("word_count"), &verified_tool(), "hello");
            assert!(!report.destroyed);
            assert!(!report.executed);
            assert!(report.memory_update.is_none());
            assert_eq!(forge.memory.len(), 0);
        }
    }

    #[test]
    fn supervisor_passes_request_limits_to_executor() {
        struct LimitExecutor;
        impl WasmExecutor for LimitExecutor {
            fn execute(&self, _: &[u8], _: &str) -> ExecuteOutcome {
                panic!("must use request limits")
            }
            fn execute_limited(
                &self,
                _: &[u8],
                _: &str,
                runtime: u64,
                memory: u64,
            ) -> ExecuteOutcome {
                assert_eq!((runtime, memory), (123, 17));
                ExecuteOutcome::Success {
                    stdout: "ok".into(),
                    execution_ms: 1,
                }
            }
        }
        let mut req = clean_req("word_count");
        req.constraints.max_runtime_ms = 123;
        req.constraints.max_memory_mb = 17;
        let mut forge = WasmForge::with_deps(
            Box::new(MockCompiler(CompileOutcome::Success {
                wasm_bytes: MOCK_WASM.to_vec(),
                compilation_ms: 1,
            })),
            Box::new(LimitExecutor),
        );
        assert!(forge.handle(&req, &verified_tool(), "hello").executed);
    }

    #[test]
    #[ignore = "requires operator-provisioned Linux isolation tools, WASI target and Wasmtime"]
    fn real_wasi_runtime_enforces_isolation_and_limits() {
        fn compile(source: &str) -> Vec<u8> {
            match RustcCompiler.compile(source) {
                CompileOutcome::Success { wasm_bytes, .. } => wasm_bytes,
                other => panic!("isolated WASI compilation failed: {other:?}"),
            }
        }
        let clean = compile(
            "pub fn run(input: &str) -> Result<String, String> { Ok(input.to_uppercase()) }",
        );
        match WasmtimeCli.execute_limited(&clean, "hello", 2000, 64) {
            ExecuteOutcome::Success { stdout, .. } => assert_eq!(stdout, "HELLO"),
            other => panic!("ordinary WASI execution failed: {other:?}"),
        }
        let observed = Workspace::new().unwrap();
        assert!(matches!(
            execute_isolated(&clean, "hello", 2000, 64, &observed),
            ExecuteOutcome::Success { .. }
        ));
        let artifacts: Vec<_> = std::fs::read_dir(&observed.path)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            artifacts,
            vec![std::ffi::OsString::from("tool.wasm")],
            "runtime must not persist compiled-module caches"
        );
        observed.cleanup().unwrap();
        let marker = Workspace::new().unwrap();
        marker
            .write("host-canary", b"SYNTHETIC_FILE_CANARY")
            .unwrap();
        let source = format!(
            "pub fn run(_: &str) -> Result<String, String> {{ Ok(include_str!({:?}).into()) }}",
            marker.path.join("host-canary")
        );
        assert!(matches!(
            RustcCompiler.compile(&source),
            CompileOutcome::CompilationFailed { .. }
        ));
        let runtime_read = compile(&format!("pub fn run(_: &str) -> Result<String, String> {{ Ok(if std::fs::read_to_string({:?}).is_err() {{ \"denied\" }} else {{ \"leaked\" }}.into()) }}", marker.path.join("host-canary")));
        match WasmtimeCli.execute_limited(&runtime_read, "", 2000, 64) {
            ExecuteOutcome::Success { stdout, .. } => assert_eq!(stdout, "denied"),
            other => panic!("guest filesystem isolation check failed: {other:?}"),
        }
        let environment = compile("pub fn run(_: &str) -> Result<String, String> { Ok(format!(\"{}|{}\", option_env!(\"SGAIL_RUNTIME_CANARY\").unwrap_or(\"absent\"), std::env::var(\"SGAIL_RUNTIME_CANARY\").unwrap_or_else(|_| \"absent\".into()))) }");
        match WasmtimeCli.execute_limited(&environment, "", 2000, 64) {
            ExecuteOutcome::Success { stdout, .. } => assert_eq!(stdout, "absent|absent"),
            other => panic!("environment isolation check failed: {other:?}"),
        }
        let spin = compile(
            "pub fn run(_: &str) -> Result<String, String> { loop { std::hint::spin_loop(); } }",
        );
        let start = Instant::now();
        let timeout = WasmtimeCli.execute_limited(&spin, "", 200, 64);
        eprintln!("spin result: {timeout:?}");
        assert!(
            matches!(timeout, ExecuteOutcome::RuntimeError { ref error } if error.contains("deadline"))
                || matches!(timeout, ExecuteOutcome::ExecutionFailed { ref stderr, .. } if stderr.contains("interrupt") || stderr.contains("timeout"))
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let memory = compile("pub fn run(_: &str) -> Result<String, String> { let data = vec![1u8; 16 * 1024 * 1024]; std::hint::black_box(&data); Ok(\"unexpected\".into()) }");
        let memory_error = WasmtimeCli.execute_limited(&memory, "", 2000, 8);
        eprintln!("memory result: {memory_error:?}");
        assert!(
            matches!(memory_error, ExecuteOutcome::ExecutionFailed { ref stderr, .. } if stderr.contains("memory") || stderr.contains("unreachable"))
        );
        let flood = compile(
            "pub fn run(_: &str) -> Result<String, String> { Ok(\"x\".repeat(1_000_000)) }",
        );
        assert!(
            matches!(WasmtimeCli.execute_limited(&flood, "", 2000, 64), ExecuteOutcome::RuntimeError { ref error } if error.contains("output limit"))
        );
        let malformed = WasmtimeCli.execute_limited(b"not wasm", "", 2000, 64);
        assert!(matches!(malformed, ExecuteOutcome::ExecutionFailed { .. }));
    }

    #[test]
    fn runtime_startup_slack_is_bounded() {
        let deadline = runtime_deadline(200);
        assert_eq!(deadline, Duration::from_millis(1_200));
        let mut near_limit = Command::new("/bin/sh");
        // Emulate startup followed by useful execution beyond the guest budget.
        near_limit.args(["-c", "sleep 0.15; sleep 0.10; printf ok"]);
        let output = process_boundary::run(&mut near_limit, b"", deadline, 1024).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"ok");
        let mut hung = Command::new("/bin/sh");
        hung.args(["-c", "sleep 20"]);
        let start = Instant::now();
        let error = process_boundary::run(&mut hung, b"", deadline, 1024).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        // Scheduling/kill/reap tolerance is test-only, not extra production slack.
        assert!(start.elapsed() < deadline + Duration::from_millis(500));
        assert_eq!(
            runtime_deadline(WASM_EXEC_TIMEOUT_MS),
            Duration::from_secs(11)
        );
    }

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
        let big = "x".repeat(sbh_core::input_validation::MAX_FORGE_INPUT_BYTES + 1);
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
