/// In-memory capability memory store.
///
/// After each successful run the supervisor stores a fingerprint — the problem
/// signature, solution pattern, and performance metrics. Generated binaries are
/// never stored; the pattern is regenerated and reverified on every future use.
use crate::capability::{CapabilityMemoryRecord, CapabilityRequest, ToolMetrics};

/// Running performance metrics accumulated across all runs of one pattern.
#[derive(Debug, Clone, Default)]
pub struct PatternMetrics {
    pub runs: u64,
    pub successes: u64,
    pub total_runtime_ms: u64,
    pub total_input_bytes: usize,
    pub total_output_bytes: usize,
    /// Current unbroken streak of failures. Resets to 0 on any success.
    pub consecutive_failures: u64,
}

impl PatternMetrics {
    pub fn record(&mut self, metrics: &ToolMetrics) {
        self.runs += 1;
        if metrics.success {
            self.successes += 1;
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures += 1;
        }
        self.total_runtime_ms += metrics.runtime_ms;
        self.total_input_bytes += metrics.input_bytes;
        self.total_output_bytes += metrics.output_bytes;
    }

    pub fn success_rate(&self) -> f64 {
        if self.runs == 0 {
            return 0.0;
        }
        self.successes as f64 / self.runs as f64
    }

    pub fn avg_runtime_ms(&self) -> f64 {
        if self.runs == 0 {
            return 0.0;
        }
        self.total_runtime_ms as f64 / self.runs as f64
    }
}

/// One entry in capability memory.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub record: CapabilityMemoryRecord,
    pub metrics: PatternMetrics,
}

/// In-memory store. Keyed by problem_signature.
/// Phase 5 will add persistence; Phase 2 keeps it in RAM.
#[derive(Debug, Default)]
pub struct CapabilityMemory {
    entries: Vec<MemoryEntry>,
}

impl CapabilityMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a prior run by problem signature.
    pub fn lookup(&self, signature: &str) -> Option<&MemoryEntry> {
        self.entries
            .iter()
            .find(|e| e.record.problem_signature == signature)
    }

    /// Insert or update an entry, accumulating metrics.
    pub fn upsert(&mut self, record: CapabilityMemoryRecord, metrics: &ToolMetrics) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.record.problem_signature == record.problem_signature)
        {
            entry.record = record;
            entry.metrics.record(metrics);
        } else {
            let mut pm = PatternMetrics::default();
            pm.record(metrics);
            self.entries.push(MemoryEntry {
                record,
                metrics: pm,
            });
        }
    }

    /// Total number of distinct patterns stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Derive a stable problem signature from a capability request.
    /// The signature is capability + input shape + output shape, lowercased and
    /// hyphen-joined. It is not a hash — it is human-readable and stable.
    pub fn derive_signature(req: &CapabilityRequest) -> String {
        let cap = req.capability.to_lowercase().replace(' ', "_");
        let inp = shape_token(&req.input_contract);
        let out = shape_token(&req.output_contract);
        format!("{cap}:{inp}:{out}")
    }
}

/// Reduce a contract description to a short shape token for use in signatures.
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

    fn make_record(sig: &str) -> CapabilityMemoryRecord {
        CapabilityMemoryRecord {
            problem_signature: sig.into(),
            solution_pattern: "mock".into(),
            input_shape: "utf8_lines".into(),
            output_shape: "json_counts".into(),
            constraints: CapabilityConstraints::default(),
        }
    }

    fn ok_metrics() -> ToolMetrics {
        ToolMetrics {
            runtime_ms: 10,
            input_bytes: 100,
            output_bytes: 50,
            success: true,
        }
    }

    #[test]
    fn lookup_returns_none_when_empty() {
        let mem = CapabilityMemory::new();
        assert!(mem.lookup("anything").is_none());
    }

    #[test]
    fn upsert_then_lookup() {
        let mut mem = CapabilityMemory::new();
        mem.upsert(make_record("test:sig"), &ok_metrics());
        assert!(mem.lookup("test:sig").is_some());
    }

    #[test]
    fn upsert_accumulates_metrics() {
        let mut mem = CapabilityMemory::new();
        mem.upsert(make_record("sig"), &ok_metrics());
        mem.upsert(make_record("sig"), &ok_metrics());
        let entry = mem.lookup("sig").unwrap();
        assert_eq!(entry.metrics.runs, 2);
        assert_eq!(entry.metrics.successes, 2);
        assert_eq!(entry.metrics.total_runtime_ms, 20);
    }

    #[test]
    fn upsert_different_sigs_stored_separately() {
        let mut mem = CapabilityMemory::new();
        mem.upsert(make_record("a"), &ok_metrics());
        mem.upsert(make_record("b"), &ok_metrics());
        assert_eq!(mem.len(), 2);
    }

    #[test]
    fn success_rate_correct() {
        let mut pm = PatternMetrics::default();
        pm.record(&ToolMetrics {
            success: true,
            ..Default::default()
        });
        pm.record(&ToolMetrics {
            success: false,
            ..Default::default()
        });
        assert!((pm.success_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn derive_signature_is_stable() {
        let req = CapabilityRequest {
            kind: "capability_request".into(),
            capability: "stream_parse_logs".into(),
            input_contract: "UTF-8 log lines from stdin".into(),
            output_contract: "JSON array of matching events".into(),
            constraints: CapabilityConstraints::default(),
            reason: "test".into(),
        };
        let s1 = CapabilityMemory::derive_signature(&req);
        let s2 = CapabilityMemory::derive_signature(&req);
        assert_eq!(s1, s2);
        assert!(s1.starts_with("stream_parse_logs:"));
    }

    #[test]
    fn derive_signature_different_contracts_differ() {
        let req_a = CapabilityRequest {
            kind: "capability_request".into(),
            capability: "parse".into(),
            input_contract: "utf8 text".into(),
            output_contract: "json counts".into(),
            constraints: CapabilityConstraints::default(),
            reason: "r".into(),
        };
        let req_b = CapabilityRequest {
            input_contract: "binary blob".into(),
            ..req_a.clone()
        };
        assert_ne!(
            CapabilityMemory::derive_signature(&req_a),
            CapabilityMemory::derive_signature(&req_b)
        );
    }
}
