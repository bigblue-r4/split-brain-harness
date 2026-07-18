//! Pure-rules arbitrator for the v1.5 active-reconciliation loop.
//!
//! After each propose→verify pass, the harness asks the arbitrator whether to
//! **accept** the current read, **re-refine** (propose again with verifier
//! feedback), or **escalate** (surface the best attempt and force stop_and_ask).
//!
//! This is deterministic — no LLM call, no added latency or cost. An `Llm`
//! adjudicator variant is reserved for v2 (a template already exists in
//! `verifier::run_reconcile`). The scoring style mirrors the threshold logic in
//! `reputation.rs`: cheap, explainable, and stable across runs.

use crate::types::{ArbiterDecision, ArbiterVerdict, RefinementIteration};

/// Decide what to do given the iterations completed so far.
///
/// * `target`    — confidence at/above which a clean iteration is accepted.
/// * `max_iters` — the loop budget (total passes allowed, including the first).
///
/// Rules, in order:
/// 1. If the latest iteration passed, is confident enough, and does not itself
///    demand stop_and_ask → **Accept** it.
/// 2. Else if budget remains → **ReRefine**.
/// 3. Else (budget exhausted) pick the highest-confidence iteration: **Accept**
///    it if it clears the bar, otherwise **Escalate** it (force stop_and_ask).
///
/// Choosing the best iteration in step 3 means a refinement that *regresses*
/// (lower confidence than an earlier pass) never costs us the earlier, better read.
pub fn decide(iters: &[RefinementIteration], target: f32, max_iters: usize) -> ArbiterDecision {
    debug_assert!(!iters.is_empty(), "arbitrator called with no iterations");
    if iters.is_empty() {
        return ArbiterDecision {
            verdict: ArbiterVerdict::Escalate,
            chosen_iteration: 0,
            reasoning: "no iterations produced — escalating".into(),
        };
    }

    let last_idx = iters.len() - 1;
    let last = &iters[last_idx];

    let acceptable =
        |it: &RefinementIteration| it.passed && it.confidence >= target && !it.stop_and_ask;

    // 1. Latest read is clean and confident — take it.
    if acceptable(last) {
        return ArbiterDecision {
            verdict: ArbiterVerdict::Accept,
            chosen_iteration: last_idx,
            reasoning: format!(
                "iteration {last_idx} passed at confidence {:.2} >= target {:.2}",
                last.confidence, target
            ),
        };
    }

    // 2. Still unresolved but budget remains — try again.
    if iters.len() < max_iters {
        return ArbiterDecision {
            verdict: ArbiterVerdict::ReRefine,
            chosen_iteration: last_idx,
            reasoning: format!(
                "iteration {last_idx} unresolved (confidence {:.2} < target {:.2} or flagged) — refining with verifier feedback",
                last.confidence, target
            ),
        };
    }

    // 3. Budget exhausted — surface the best attempt.
    let best_idx = best_iteration(iters);
    let best = &iters[best_idx];
    if acceptable(best) {
        ArbiterDecision {
            verdict: ArbiterVerdict::Accept,
            chosen_iteration: best_idx,
            reasoning: format!(
                "budget exhausted; best iteration {best_idx} is acceptable at confidence {:.2}",
                best.confidence
            ),
        }
    } else {
        ArbiterDecision {
            verdict: ArbiterVerdict::Escalate,
            chosen_iteration: best_idx,
            reasoning: format!(
                "budget exhausted without resolution — escalating best iteration {best_idx} (confidence {:.2})",
                best.confidence
            ),
        }
    }
}

/// Index of the highest-confidence iteration; ties resolve to the earliest.
fn best_iteration(iters: &[RefinementIteration]) -> usize {
    let mut best_idx = 0;
    for (i, it) in iters.iter().enumerate().skip(1) {
        if it.confidence > iters[best_idx].confidence {
            best_idx = i;
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(
        iteration: usize,
        confidence: f32,
        passed: bool,
        stop_and_ask: bool,
    ) -> RefinementIteration {
        RefinementIteration {
            iteration,
            confidence,
            passed,
            stop_and_ask,
            flag_count: if passed { 0 } else { 1 },
        }
    }

    #[test]
    fn accepts_clean_first_pass() {
        let iters = vec![it(0, 0.9, true, false)];
        let d = decide(&iters, 0.4, 2);
        assert_eq!(d.verdict, ArbiterVerdict::Accept);
        assert_eq!(d.chosen_iteration, 0);
    }

    #[test]
    fn re_refines_when_flagged_and_budget_remains() {
        let iters = vec![it(0, 0.3, false, true)];
        let d = decide(&iters, 0.4, 2);
        assert_eq!(d.verdict, ArbiterVerdict::ReRefine);
    }

    #[test]
    fn accepts_improved_second_pass() {
        let iters = vec![it(0, 0.3, false, true), it(1, 0.8, true, false)];
        let d = decide(&iters, 0.4, 2);
        assert_eq!(d.verdict, ArbiterVerdict::Accept);
        assert_eq!(d.chosen_iteration, 1);
    }

    #[test]
    fn escalates_when_budget_exhausted_unresolved() {
        let iters = vec![it(0, 0.3, false, true), it(1, 0.35, false, true)];
        let d = decide(&iters, 0.4, 2);
        assert_eq!(d.verdict, ArbiterVerdict::Escalate);
        // best is iteration 1 (higher confidence)
        assert_eq!(d.chosen_iteration, 1);
    }

    #[test]
    fn regression_keeps_the_better_earlier_iteration() {
        // Second pass is WORSE than the first; neither clears the bar.
        let iters = vec![it(0, 0.38, false, true), it(1, 0.10, false, true)];
        let d = decide(&iters, 0.4, 2);
        assert_eq!(d.verdict, ArbiterVerdict::Escalate);
        assert_eq!(
            d.chosen_iteration, 0,
            "must not surface the regressed second pass"
        );
    }
}
