/// Static policy layer — checks a CapabilityRequest against hard rules before
/// the supervisor allows any execution. All checks are deterministic and cheap.
///
/// Rules enforced here:
/// - network access is forbidden (no_network must be true)
/// - filesystem writes are forbidden (read_only_input must be true)
/// - resource limits must not exceed supervisor-side maximums
/// - the request must pass its own validate() check
use crate::capability::{Budget, CapabilityRequest, PolicyViolation, ToolMetrics};

/// Hard-coded ceilings the supervisor will never negotiate past.
const MAX_RUNTIME_MS: u64 = 10_000;
const MAX_MEMORY_MB: u64 = 256;

/// Per-session accounting state tracked alongside the Budget.
#[derive(Debug, Default)]
pub struct PolicyState {
    pub tools_invoked: usize,
    pub consecutive_failures: usize,
    pub total_runtime_ms: u64,
}

impl PolicyState {
    /// Update state after a completed run.
    pub fn record_run(&mut self, metrics: &ToolMetrics) {
        self.tools_invoked += 1;
        self.total_runtime_ms += metrics.runtime_ms;
        if metrics.success {
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures += 1;
        }
    }

    /// Returns Some(reason) if any budget limit is already exhausted.
    pub fn budget_exceeded(&self, budget: &Budget) -> Option<String> {
        if self.tools_invoked >= budget.max_tools_per_session {
            return Some(format!(
                "session tool limit reached ({}/{})",
                self.tools_invoked, budget.max_tools_per_session
            ));
        }
        if self.total_runtime_ms >= budget.max_total_runtime_ms {
            return Some(format!(
                "session runtime budget exhausted ({}ms/{}ms)",
                self.total_runtime_ms, budget.max_total_runtime_ms
            ));
        }
        if self.consecutive_failures >= budget.require_approval_after_failures {
            return Some(format!(
                "{} consecutive failures — user approval required before continuing",
                self.consecutive_failures
            ));
        }
        None
    }
}

/// Run all static policy checks against a request. Returns a list of violations;
/// an empty list means the request is clean.
pub fn check_request(req: &CapabilityRequest) -> Vec<PolicyViolation> {
    let mut violations: Vec<PolicyViolation> = vec![];

    // Structural validity
    if let Err(e) = req.validate() {
        violations.push(PolicyViolation {
            rule: "structural_validity".into(),
            detail: e,
        });
    }

    // Network access
    if !req.constraints.no_network {
        violations.push(PolicyViolation {
            rule: "no_network".into(),
            detail: "capability_request.constraints.no_network must be true".into(),
        });
    }

    // Read-only input
    if !req.constraints.read_only_input {
        violations.push(PolicyViolation {
            rule: "read_only_input".into(),
            detail: "capability_request.constraints.read_only_input must be true".into(),
        });
    }

    // Runtime ceiling
    if req.constraints.max_runtime_ms > MAX_RUNTIME_MS {
        violations.push(PolicyViolation {
            rule: "max_runtime_ms".into(),
            detail: format!(
                "requested {}ms exceeds supervisor ceiling of {}ms",
                req.constraints.max_runtime_ms, MAX_RUNTIME_MS
            ),
        });
    }

    // Memory ceiling
    if req.constraints.max_memory_mb > MAX_MEMORY_MB {
        violations.push(PolicyViolation {
            rule: "max_memory_mb".into(),
            detail: format!(
                "requested {}MB exceeds supervisor ceiling of {}MB",
                req.constraints.max_memory_mb, MAX_MEMORY_MB
            ),
        });
    }

    violations
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityConstraints;

    fn clean_request() -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: "test_cap".into(),
            input_contract: "utf8 text".into(),
            output_contract: "json".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning insufficient".into(),
        }
    }

    #[test]
    fn clean_request_has_no_violations() {
        let req = clean_request();
        assert!(check_request(&req).is_empty());
    }

    #[test]
    fn network_access_rejected() {
        let mut req = clean_request();
        req.constraints.no_network = false;
        let v = check_request(&req);
        assert!(v.iter().any(|v| v.rule == "no_network"));
    }

    #[test]
    fn non_readonly_rejected() {
        let mut req = clean_request();
        req.constraints.read_only_input = false;
        let v = check_request(&req);
        assert!(v.iter().any(|v| v.rule == "read_only_input"));
    }

    #[test]
    fn excessive_runtime_rejected() {
        let mut req = clean_request();
        req.constraints.max_runtime_ms = 99_999;
        let v = check_request(&req);
        assert!(v.iter().any(|v| v.rule == "max_runtime_ms"));
    }

    #[test]
    fn excessive_memory_rejected() {
        let mut req = clean_request();
        req.constraints.max_memory_mb = 512;
        let v = check_request(&req);
        assert!(v.iter().any(|v| v.rule == "max_memory_mb"));
    }

    #[test]
    fn invalid_kind_rejected() {
        let mut req = clean_request();
        req.kind = "wrong".into();
        let v = check_request(&req);
        assert!(v.iter().any(|v| v.rule == "structural_validity"));
    }

    #[test]
    fn budget_exceeded_on_tool_limit() {
        let budget = Budget {
            max_tools_per_session: 2,
            ..Budget::default()
        };
        let mut state = PolicyState::default();
        state.tools_invoked = 2;
        assert!(state.budget_exceeded(&budget).is_some());
    }

    #[test]
    fn budget_ok_under_limit() {
        let budget = Budget::default();
        let state = PolicyState::default();
        assert!(state.budget_exceeded(&budget).is_none());
    }

    #[test]
    fn consecutive_failures_trigger_approval() {
        let budget = Budget {
            require_approval_after_failures: 2,
            ..Budget::default()
        };
        let mut state = PolicyState::default();
        state.consecutive_failures = 2;
        let reason = state.budget_exceeded(&budget).unwrap();
        assert!(reason.contains("approval"));
    }

    #[test]
    fn success_resets_consecutive_failures() {
        let mut state = PolicyState {
            consecutive_failures: 3,
            ..Default::default()
        };
        state.record_run(&ToolMetrics {
            success: true,
            runtime_ms: 10,
            ..Default::default()
        });
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn failure_increments_consecutive_failures() {
        let mut state = PolicyState::default();
        state.record_run(&ToolMetrics {
            success: false,
            runtime_ms: 5,
            ..Default::default()
        });
        assert_eq!(state.consecutive_failures, 1);
    }
}
