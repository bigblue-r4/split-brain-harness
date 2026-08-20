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

use crate::backends::{Engines, InferenceEngine, Role};

pub struct MeteredEngine<'e> {
    inner: &'e dyn InferenceEngine,
    /// Per-role engine set, when the harness was built with one. The budget is
    /// deliberately held here and not per role: a split-brain run must not get
    /// three times the call ceiling just because it uses three engines.
    engines: Option<&'e Engines>,
    /// Per-request ceiling. `None` = unlimited (still counted).
    limit: Option<usize>,
    used: AtomicUsize,
}

impl<'e> MeteredEngine<'e> {
    pub fn new(inner: &'e dyn InferenceEngine, limit: Option<usize>) -> Self {
        Self {
            inner,
            engines: None,
            limit,
            used: AtomicUsize::new(0),
        }
    }

    /// Meter a per-role engine set under one shared budget.
    pub fn new_roles(engines: &'e Engines, limit: Option<usize>) -> Self {
        Self {
            inner: engines.for_role(Role::Proposer),
            engines: Some(engines),
            limit,
            used: AtomicUsize::new(0),
        }
    }

    /// A view that routes to `role`'s engine while charging this meter's budget.
    /// Without a role set every view resolves to the single wrapped engine, so
    /// callers can hand out views unconditionally.
    pub fn role_view(&self, role: Role) -> RoleView<'_, 'e> {
        RoleView {
            parent: self,
            engine: match self.engines {
                Some(e) => e.for_role(role),
                None => self.inner,
            },
        }
    }

    /// Reserve one call against the budget. `Err` when the ceiling is reached;
    /// a refused call is not counted.
    fn charge(&self) -> Result<(), String> {
        let cur = self.used.load(Ordering::Relaxed);
        if let Some(limit) = self.limit {
            if cur >= limit {
                return Err(format!(
                    "LLM call budget exceeded: per-request limit of {limit} reached"
                ));
            }
        }
        self.used.store(cur + 1, Ordering::Relaxed);
        Ok(())
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
        self.charge()?;
        self.inner.generate(system_prompt, prompt_payload).await
    }
}

/// One role's engine, metered against the parent `MeteredEngine`'s budget.
pub struct RoleView<'m, 'e> {
    parent: &'m MeteredEngine<'e>,
    engine: &'e dyn InferenceEngine,
}

#[async_trait]
impl InferenceEngine for RoleView<'_, '_> {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String> {
        self.parent.charge()?;
        self.engine.generate(system_prompt, prompt_payload).await
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

    /// Names which engine answered, so a role view's routing is observable.
    struct NamedEngine(&'static str);
    #[async_trait]
    impl InferenceEngine for NamedEngine {
        async fn generate(&self, _s: &str, _p: &str) -> Result<String, String> {
            Ok(self.0.into())
        }
    }

    fn split_engines() -> Engines {
        Engines::new(
            Box::new(NamedEngine("proposer")),
            Some(Box::new(NamedEngine("verifier"))),
            Some(Box::new(NamedEngine("adjudicator"))),
        )
    }

    #[tokio::test]
    async fn role_views_route_to_their_own_engine() {
        let engines = split_engines();
        let m = MeteredEngine::new_roles(&engines, None);
        assert_eq!(
            m.role_view(Role::Proposer)
                .generate("s", "p")
                .await
                .unwrap(),
            "proposer"
        );
        assert_eq!(
            m.role_view(Role::Verifier)
                .generate("s", "p")
                .await
                .unwrap(),
            "verifier"
        );
        assert_eq!(
            m.role_view(Role::Adjudicator)
                .generate("s", "p")
                .await
                .unwrap(),
            "adjudicator"
        );
    }

    #[tokio::test]
    async fn all_roles_charge_one_shared_budget() {
        let engines = split_engines();
        let m = MeteredEngine::new_roles(&engines, Some(2));
        // Three engines, one ceiling — a split-brain run must not get 3x the budget.
        m.role_view(Role::Proposer)
            .generate("s", "p")
            .await
            .unwrap();
        m.role_view(Role::Verifier)
            .generate("s", "p")
            .await
            .unwrap();
        assert_eq!(m.used(), 2);
        assert!(!m.has_budget());
        let err = m
            .role_view(Role::Adjudicator)
            .generate("s", "p")
            .await
            .unwrap_err();
        assert!(err.contains("budget exceeded"));
        assert_eq!(m.used(), 2, "a refused call is not counted");
    }

    #[tokio::test]
    async fn role_views_without_an_engine_set_all_use_the_single_engine() {
        let inner = NamedEngine("only");
        let m = MeteredEngine::new(&inner, None);
        for role in [Role::Proposer, Role::Verifier, Role::Adjudicator] {
            assert_eq!(m.role_view(role).generate("s", "p").await.unwrap(), "only");
        }
        assert_eq!(m.used(), 3);
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
