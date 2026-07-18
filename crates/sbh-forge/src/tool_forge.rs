/// Phase 2 mock tool forge — the supervisor.
///
/// The supervisor:
///   1. Checks the budget (session limits).
///   2. Runs static policy checks (no network, read-only, resource ceilings).
///   3. Looks up the capability name in the mock registry.
///   4. Executes the mock (deterministic, no generated code).
///   5. Updates capability memory.
///   6. Returns a ToolRunReport.
///
/// The model never runs code. The model emits a CapabilityRequest; the
/// supervisor decides, runs, and destroys.
use std::collections::HashMap;
use std::time::Instant;

use crate::tool_memory::CapabilityMemory;
use sbh_core::capability::{
    Budget, CapabilityMemoryRecord, CapabilityRequest, ToolMetrics, ToolRunReport,
};
use sbh_core::input_validation;
use sbh_safety::policy::{self, PolicyState};

/// Signature for a mock tool implementation.
///
/// Arguments:
/// - `input` — the original user input text passed to the harness
/// - `req`   — the parsed CapabilityRequest from the model
///
/// Returns a serialized JSON string on success, or an error description.
pub type MockToolFn = fn(input: &str, req: &CapabilityRequest) -> Result<String, String>;

/// Registry of hand-written mock tool implementations keyed by capability name.
pub struct MockToolRegistry {
    tools: HashMap<&'static str, MockToolFn>,
}

impl MockToolRegistry {
    fn new() -> Self {
        let mut tools: HashMap<&'static str, MockToolFn> = HashMap::new();
        tools.insert("stream_parse_logs", mock_stream_parse_logs);
        tools.insert("word_count", mock_word_count);
        tools.insert("json_extract", mock_json_extract);
        Self { tools }
    }

    pub fn get(&self, name: &str) -> Option<MockToolFn> {
        self.tools.get(name).copied()
    }

    pub fn known_capabilities(&self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = self.tools.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

/// The supervisor. Owns session state (budget accounting + capability memory).
/// One Forge per session; each call to `handle` uses up budget.
pub struct Forge {
    budget: Budget,
    state: PolicyState,
    registry: MockToolRegistry,
    pub memory: CapabilityMemory,
    /// Immutable record of every decision made this session.
    session_log: Vec<ToolRunReport>,
}

impl Forge {
    pub fn new() -> Self {
        Self {
            budget: Budget::default(),
            state: PolicyState::default(),
            registry: MockToolRegistry::new(),
            memory: CapabilityMemory::new(),
            session_log: vec![],
        }
    }

    pub fn with_budget(budget: Budget) -> Self {
        Self {
            budget,
            state: PolicyState::default(),
            registry: MockToolRegistry::new(),
            memory: CapabilityMemory::new(),
            session_log: vec![],
        }
    }

    /// Immutable record of every ToolRunReport produced this session,
    /// in order. Includes accepted and rejected requests.
    pub fn audit(&self) -> &[ToolRunReport] {
        &self.session_log
    }

    /// Process one CapabilityRequest from the model.
    ///
    /// Decision order:
    ///   input validation → budget check → policy check → registry lookup
    ///   → execute → memory update → audit log
    ///
    /// Every call is recorded in `self.session_log` regardless of outcome.
    pub fn handle(&mut self, req: &CapabilityRequest, input: &str) -> ToolRunReport {
        let report = self.handle_inner(req, input);
        self.session_log.push(report.clone());
        report
    }

    fn handle_inner(&mut self, req: &CapabilityRequest, input: &str) -> ToolRunReport {
        // Validate forge input — reject malformed strings before any processing
        if let Err(e) = input_validation::validate_forge_input(input) {
            return rejected(vec![format!("input validation: {e}")]);
        }

        // Validate capability request fields — length limits on model-supplied strings
        if let Err(e) = input_validation::validate_capability_fields(req) {
            return rejected(vec![format!("capability field validation: {e}")]);
        }

        // Budget check — fail fast if session limits are already exhausted
        if let Some(reason) = self.state.budget_exceeded(&self.budget) {
            return rejected(vec![reason]);
        }

        // Static policy checks
        let violations = policy::check_request(req);
        if !violations.is_empty() {
            return rejected(violations.into_iter().map(|v| v.detail).collect());
        }

        // Registry lookup — explicit allowlist of known capabilities
        let mock_fn = match self.registry.get(&req.capability) {
            Some(f) => f,
            None => {
                return rejected(vec![format!(
                    "capability '{}' is not registered; known: {}",
                    req.capability,
                    self.registry.known_capabilities().join(", ")
                )]);
            }
        };

        // Execute
        let start = Instant::now();
        let exec_result = mock_fn(input, req);
        let runtime_ms = start.elapsed().as_millis() as u64;

        let (output_str, success) = match exec_result {
            Ok(out) => (Some(out), true),
            Err(e) => (Some(format!("{{\"error\":\"{e}\"}}")), false),
        };

        let metrics = ToolMetrics {
            runtime_ms,
            input_bytes: input.len(),
            output_bytes: output_str.as_deref().map(|s| s.len()).unwrap_or(0),
            success,
        };

        // Record against budget
        self.state.record_run(&metrics);

        // Update capability memory on success
        let memory_update = if success {
            let signature = CapabilityMemory::derive_signature(req);
            let record = CapabilityMemoryRecord {
                problem_signature: signature,
                solution_pattern: format!("mock:{}", req.capability),
                input_shape: shape_token(&req.input_contract),
                output_shape: shape_token(&req.output_contract),
                constraints: req.constraints.clone(),
            };
            self.memory.upsert(record.clone(), &metrics);
            Some(record)
        } else {
            None
        };

        ToolRunReport {
            accepted: true,
            rejection_reasons: vec![],
            verification_passed: true, // Phase 2: mocks are pre-verified by definition
            executed: true,
            output: output_str,
            metrics,
            destroyed: true, // lifecycle complete; no binary existed to destroy
            memory_update,
        }
    }

    pub fn tools_invoked(&self) -> usize {
        self.state.tools_invoked
    }
}

impl Default for Forge {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn rejected(reasons: Vec<String>) -> ToolRunReport {
    ToolRunReport {
        accepted: false,
        rejection_reasons: reasons,
        verification_passed: false,
        executed: false,
        output: None,
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
// Mock tool implementations
// ---------------------------------------------------------------------------

/// Counts input lines and lines matching common HTTP status code patterns.
/// Returns JSON: {"total_lines": N, "status_counts": {"200": M, ...}}
fn mock_stream_parse_logs(input: &str, _req: &CapabilityRequest) -> Result<String, String> {
    let mut status_counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;

    for line in input.lines() {
        total += 1;
        // Very simple: look for a 3-digit HTTP status code after a space
        let mut found = false;
        for token in line.split_whitespace() {
            if token.len() == 3 && token.chars().all(|c| c.is_ascii_digit()) {
                let first = token.chars().next().unwrap();
                if ('1'..='5').contains(&first) {
                    *status_counts.entry(token.to_string()).or_insert(0) += 1;
                    found = true;
                    break;
                }
            }
        }
        if !found && !line.trim().is_empty() {
            *status_counts.entry("unknown".to_string()).or_insert(0) += 1;
        }
    }

    let counts_json: Vec<String> = status_counts
        .iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect();

    Ok(format!(
        "{{\"total_lines\":{total},\"status_counts\":{{{}}},\"note\":\"mock:stream_parse_logs\"}}",
        counts_json.join(",")
    ))
}

/// Counts words, lines, and characters in the input text.
/// Returns JSON: {"word_count": N, "line_count": N, "char_count": N}
fn mock_word_count(input: &str, _req: &CapabilityRequest) -> Result<String, String> {
    let word_count = input.split_whitespace().count();
    let line_count = input.lines().count();
    let char_count = input.chars().count();
    Ok(format!(
        "{{\"word_count\":{word_count},\"line_count\":{line_count},\"char_count\":{char_count},\"note\":\"mock:word_count\"}}"
    ))
}

/// Parses input as JSON and returns the top-level field names.
/// Returns JSON: {"fields": ["a", "b", ...]}
fn mock_json_extract(input: &str, _req: &CapabilityRequest) -> Result<String, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(input).map_err(|e| format!("json parse error: {e}"))?;

    let fields: Vec<String> = match &parsed {
        serde_json::Value::Object(map) => map.keys().map(|k| format!("\"{k}\"")).collect(),
        _ => return Err("input must be a JSON object".into()),
    };

    Ok(format!(
        "{{\"fields\":[{}],\"note\":\"mock:json_extract\"}}",
        fields.join(",")
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sbh_core::capability::CapabilityConstraints;

    fn clean_req(capability: &str) -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: capability.into(),
            input_contract: "utf8 text".into(),
            output_contract: "json object".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning cannot efficiently process this".into(),
        }
    }

    #[test]
    fn accepts_registered_capability_and_executes() {
        let mut forge = Forge::new();
        let req = clean_req("word_count");
        let report = forge.handle(&req, "hello world\nsecond line");
        assert!(report.accepted, "should be accepted");
        assert!(report.executed, "should have executed");
        assert!(report.destroyed, "lifecycle must be marked complete");
        assert!(report.rejection_reasons.is_empty());
        let out = report.output.unwrap();
        assert!(out.contains("word_count"));
    }

    #[test]
    fn rejects_unknown_capability() {
        let mut forge = Forge::new();
        let req = clean_req("nonexistent_tool");
        let report = forge.handle(&req, "input");
        assert!(!report.accepted);
        assert!(!report.executed);
        assert!(report.rejection_reasons[0].contains("not registered"));
    }

    #[test]
    fn rejects_network_access_request() {
        let mut forge = Forge::new();
        let mut req = clean_req("word_count");
        req.constraints.no_network = false;
        let report = forge.handle(&req, "input");
        assert!(!report.accepted);
        assert!(report
            .rejection_reasons
            .iter()
            .any(|r| r.contains("no_network")));
    }

    #[test]
    fn rejects_when_budget_exhausted() {
        let budget = Budget {
            max_tools_per_session: 1,
            ..Budget::default()
        };
        let mut forge = Forge::with_budget(budget);
        // First run consumes the budget
        forge.handle(&clean_req("word_count"), "hello");
        // Second run should be rejected
        let report = forge.handle(&clean_req("word_count"), "hello");
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("session tool limit"));
    }

    #[test]
    fn updates_memory_on_success() {
        let mut forge = Forge::new();
        let req = clean_req("word_count");
        let report = forge.handle(&req, "hello world");
        assert!(report.memory_update.is_some());
        assert!(!forge.memory.is_empty());
    }

    #[test]
    fn mock_stream_parse_logs_counts_status_codes() {
        let input = "127.0.0.1 - - [01/Jan/2025] \"GET / HTTP/1.1\" 200 1234\n\
                     127.0.0.1 - - [01/Jan/2025] \"GET /missing HTTP/1.1\" 404 0\n\
                     127.0.0.1 - - [01/Jan/2025] \"POST /api HTTP/1.1\" 200 500";
        let mut forge = Forge::new();
        let req = clean_req("stream_parse_logs");
        let report = forge.handle(&req, input);
        assert!(report.accepted);
        let out = report.output.unwrap();
        assert!(out.contains("\"200\""));
        assert!(out.contains("\"404\""));
        assert!(out.contains("total_lines"));
    }

    #[test]
    fn mock_word_count_correct_counts() {
        let input = "hello world\nthird word here";
        let mut forge = Forge::new();
        let report = forge.handle(&clean_req("word_count"), input);
        assert!(report.accepted);
        let out = report.output.unwrap();
        // 5 words
        assert!(out.contains("\"word_count\":5"), "got: {out}");
        // 2 lines
        assert!(out.contains("\"line_count\":2"), "got: {out}");
    }

    #[test]
    fn mock_json_extract_returns_field_names() {
        let input = r#"{"alpha": 1, "beta": "two"}"#;
        let mut forge = Forge::new();
        let report = forge.handle(&clean_req("json_extract"), input);
        assert!(report.accepted);
        let out = report.output.unwrap();
        assert!(out.contains("alpha") && out.contains("beta"), "got: {out}");
    }

    #[test]
    fn mock_json_extract_error_on_non_object() {
        let input = "[1, 2, 3]";
        let mut forge = Forge::new();
        let report = forge.handle(&clean_req("json_extract"), input);
        assert!(report.accepted, "accepted — mock ran to completion");
        assert!(!report.metrics.success, "but execution failed");
    }

    #[test]
    fn budget_tracks_multiple_runs() {
        let mut forge = Forge::new();
        forge.handle(&clean_req("word_count"), "a");
        forge.handle(&clean_req("word_count"), "b");
        assert_eq!(forge.tools_invoked(), 2);
    }

    #[test]
    fn memory_accumulates_across_runs() {
        let mut forge = Forge::new();
        forge.handle(&clean_req("word_count"), "first input");
        forge.handle(&clean_req("word_count"), "second input");
        // Same signature — should be one entry with 2 runs
        assert_eq!(forge.memory.len(), 1);
        let sig = CapabilityMemory::derive_signature(&clean_req("word_count"));
        let entry = forge.memory.lookup(&sig).unwrap();
        assert_eq!(entry.metrics.runs, 2);
    }

    // --- Input validation at the forge boundary ---

    #[test]
    fn forge_rejects_oversized_input() {
        let mut forge = Forge::new();
        let big = "x".repeat(sbh_core::input_validation::MAX_FORGE_INPUT_BYTES + 1);
        let report = forge.handle(&clean_req("word_count"), &big);
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("input validation"));
    }

    #[test]
    fn forge_rejects_null_byte_in_input() {
        let mut forge = Forge::new();
        let report = forge.handle(&clean_req("word_count"), "good\x00bad");
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("input validation"));
    }

    #[test]
    fn forge_rejects_oversized_capability_name() {
        let mut forge = Forge::new();
        let mut req = clean_req("word_count");
        req.capability = "x".repeat(sbh_core::input_validation::MAX_CAPABILITY_NAME_BYTES + 1);
        let report = forge.handle(&req, "hello");
        assert!(!report.accepted);
        assert!(report.rejection_reasons[0].contains("capability field validation"));
    }

    // --- Session audit log ---

    #[test]
    fn session_log_records_all_calls() {
        let mut forge = Forge::new();
        forge.handle(&clean_req("word_count"), "a");
        forge.handle(&clean_req("nonexistent"), "b");
        assert_eq!(
            forge.audit().len(),
            2,
            "both calls must appear in audit log"
        );
    }

    #[test]
    fn session_log_records_rejections() {
        let mut forge = Forge::new();
        let mut req = clean_req("word_count");
        req.constraints.no_network = false;
        forge.handle(&req, "input");
        let log = forge.audit();
        assert_eq!(log.len(), 1);
        assert!(!log[0].accepted, "rejected call must be in audit log");
    }

    // --- Idempotent repeated calls ---

    #[test]
    fn repeated_calls_same_input_produce_same_output() {
        let mut forge = Forge::new();
        let r1 = forge.handle(&clean_req("word_count"), "hello world");
        let r2 = forge.handle(&clean_req("word_count"), "hello world");
        // Mock is deterministic — outputs should be identical
        assert_eq!(r1.output, r2.output, "mock tools must be deterministic");
    }

    // --- Failure recovery after exception ---

    #[test]
    fn failure_recovery_bad_then_good_input() {
        let mut forge = Forge::new();
        // First call: json_extract with bad JSON (fails)
        let r1 = forge.handle(&clean_req("json_extract"), "[1, 2, 3]");
        assert!(r1.accepted, "accepted — mock ran to completion");
        assert!(!r1.metrics.success, "but execution failed (not an object)");

        // Second call: different tool succeeds — forge is not corrupted
        let r2 = forge.handle(&clean_req("word_count"), "hello world");
        assert!(r2.accepted);
        assert!(
            r2.metrics.success,
            "word_count should succeed after json_extract failed"
        );
    }

    // --- Shared state isolation between Forge instances ---

    #[test]
    fn two_forge_instances_do_not_share_memory() {
        let mut forge_a = Forge::new();
        let forge_b = Forge::new();

        forge_a.handle(&clean_req("word_count"), "a");
        assert_eq!(
            forge_a.memory.len(),
            1,
            "forge_a should have 1 memory entry"
        );
        assert_eq!(
            forge_b.memory.len(),
            0,
            "forge_b memory must be independent"
        );
    }

    #[test]
    fn two_forge_instances_do_not_share_budget() {
        let budget = Budget {
            max_tools_per_session: 1,
            ..Budget::default()
        };
        let mut forge_a = Forge::with_budget(budget.clone());
        let mut forge_b = Forge::with_budget(budget);

        // Exhaust forge_a's budget
        forge_a.handle(&clean_req("word_count"), "a");
        let rejected_a = forge_a.handle(&clean_req("word_count"), "b");
        assert!(!rejected_a.accepted, "forge_a should be exhausted");

        // forge_b is unaffected
        let ok_b = forge_b.handle(&clean_req("word_count"), "c");
        assert!(
            ok_b.accepted,
            "forge_b budget must be independent of forge_a"
        );
    }
}
