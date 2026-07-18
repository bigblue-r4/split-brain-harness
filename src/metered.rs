//! Per-request LLM-call budget (phase E.2).
//!
//! `MeteredEngine` wraps the real inference engine and counts every
//! `generate()` call for one `analyze()` run. When a per-request ceiling is
//! configured, calls beyond it are refused with an error rather than executed —
//! a hard stop on call stacking (refinement × verifier × advocate) that a
//! runaway or adversarial input could otherwise drive. With no ceiling it still
//! counts, feeding the `sbh_llm_calls_total` metric and the trace.
//!
//! Calls within a single analysis are sequential (each `generate` is awaited
//! before the next), so a plain relaxed `AtomicUsize` is sufficient — no CAS
//! loop needed.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::backends::InferenceEngine;

pub struct MeteredEngine<'e> {
    inner: &'e dyn InferenceEngine,
    /// Per-request ceiling. `None` = unlimited (still counted).
    limit: Option<usize>,
    used: AtomicUsize,
}

impl<'e> MeteredEngine<'e> {
    pub fn new(inner: &'e dyn InferenceEngine, limit: Option<usize>) -> Self {
        Self {
            inner,
            limit,
            used: AtomicUsize::new(0),
        }
    }

    /// Calls made so far this request.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    /// Remaining budget, or `None` when unlimited.
    pub fn remaining(&self) -> Option<usize> {
        self.limit.map(|l| l.saturating_sub(self.used()))
    }

    /// True if at least one more call is permitted (always true when unlimited).
    pub fn has_budget(&self) -> bool {
        self.remaining().map(|r| r > 0).unwrap_or(true)
    }
}

#[async_trait]
impl InferenceEngine for MeteredEngine<'_> {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String> {
        let cur = self.used.load(Ordering::Relaxed);
        if let Some(limit) = self.limit {
            if cur >= limit {
                return Err(format!(
                    "LLM call budget exceeded: per-request limit of {limit} reached"
                ));
            }
        }
        self.used.store(cur + 1, Ordering::Relaxed);
        self.inner.generate(system_prompt, prompt_payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingEngine;
    #[async_trait]
    impl InferenceEngine for CountingEngine {
        async fn generate(&self, _s: &str, _p: &str) -> Result<String, String> {
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn counts_calls() {
        let inner = CountingEngine;
        let m = MeteredEngine::new(&inner, None);
        assert_eq!(m.used(), 0);
        assert!(m.has_budget());
        m.generate("s", "p").await.unwrap();
        m.generate("s", "p").await.unwrap();
        assert_eq!(m.used(), 2);
        assert_eq!(m.remaining(), None);
    }

    #[tokio::test]
    async fn enforces_ceiling() {
        let inner = CountingEngine;
        let m = MeteredEngine::new(&inner, Some(2));
        assert!(m.generate("s", "p").await.is_ok());
        assert!(m.generate("s", "p").await.is_ok());
        assert_eq!(m.remaining(), Some(0));
        assert!(!m.has_budget());
        let err = m.generate("s", "p").await.unwrap_err();
        assert!(err.contains("budget exceeded"));
        // A rejected call is not counted against the total.
        assert_eq!(m.used(), 2);
    }
}
