/// Reputation scoring for capability patterns.
///
/// Computes a trust score and tier from accumulated `PatternMetrics`.
/// Used by the Phase 5 `RegenerativeForge` to gate retries and blacklist
/// patterns that have failed too many times in a row.
use serde::{Deserialize, Serialize};

use crate::tool_memory::PatternMetrics;

// ---------------------------------------------------------------------------
// Thresholds
// ---------------------------------------------------------------------------

/// Consecutive failures before a pattern is blacklisted.
pub const BLACKLIST_THRESHOLD: u64 = 3;

/// Minimum runs before a pattern can reach Trusted tier.
const TRUSTED_MIN_RUNS: u64 = 5;

/// Minimum runs + minimum trust score before a pattern is Promoted.
const PROMOTED_MIN_RUNS: u64 = 10;
const PROMOTED_MIN_TRUST: f64 = 0.90;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReputationTier {
    /// Fewer than 2 runs — too little data to judge.
    Untrusted,
    /// 2–(TRUSTED_MIN_RUNS - 1) runs, or lower trust score.
    Emerging,
    /// At least TRUSTED_MIN_RUNS successful runs with reasonable trust.
    Trusted,
    /// High-volume, high-trust pattern — may receive extra retry budget.
    Promoted,
    /// BLACKLIST_THRESHOLD+ consecutive failures — rejected immediately.
    Blacklisted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationScore {
    /// Laplace-smoothed success rate in [0.0, 1.0].
    pub trust: f64,
    pub tier: ReputationTier,
    pub total_runs: u64,
    pub consecutive_failures: u64,
}

impl ReputationScore {
    pub fn is_blacklisted(&self) -> bool {
        self.tier == ReputationTier::Blacklisted
    }

    pub fn is_promoted(&self) -> bool {
        self.tier == ReputationTier::Promoted
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Compute a reputation score from accumulated pattern metrics.
/// Call `compute_unknown()` when no metrics exist yet.
pub fn compute(metrics: &PatternMetrics) -> ReputationScore {
    if metrics.consecutive_failures >= BLACKLIST_THRESHOLD {
        return ReputationScore {
            trust: 0.0,
            tier: ReputationTier::Blacklisted,
            total_runs: metrics.runs,
            consecutive_failures: metrics.consecutive_failures,
        };
    }

    // Laplace smoothing: (successes + 1) / (runs + 2) avoids 0 and 1 extremes
    let trust = if metrics.runs == 0 {
        0.5
    } else {
        (metrics.successes as f64 + 1.0) / (metrics.runs as f64 + 2.0)
    };

    let tier = if metrics.runs < 2 {
        ReputationTier::Untrusted
    } else if trust >= PROMOTED_MIN_TRUST && metrics.runs >= PROMOTED_MIN_RUNS {
        ReputationTier::Promoted
    } else if trust > 0.5 && metrics.runs >= TRUSTED_MIN_RUNS {
        ReputationTier::Trusted
    } else {
        ReputationTier::Emerging
    };

    ReputationScore {
        trust,
        tier,
        total_runs: metrics.runs,
        consecutive_failures: metrics.consecutive_failures,
    }
}

/// Reputation for a pattern that has never been seen before.
pub fn compute_unknown() -> ReputationScore {
    ReputationScore {
        trust: 0.5,
        tier: ReputationTier::Untrusted,
        total_runs: 0,
        consecutive_failures: 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ToolMetrics;

    fn metrics_from(runs: &[bool]) -> PatternMetrics {
        let mut pm = PatternMetrics::default();
        for &success in runs {
            pm.record(&ToolMetrics {
                success,
                ..Default::default()
            });
        }
        pm
    }

    #[test]
    fn unknown_is_untrusted_not_blacklisted() {
        let s = compute_unknown();
        assert_eq!(s.tier, ReputationTier::Untrusted);
        assert!(!s.is_blacklisted());
        assert!((s.trust - 0.5).abs() < 1e-9);
    }

    #[test]
    fn zero_runs_is_untrusted() {
        let s = compute(&PatternMetrics::default());
        assert_eq!(s.tier, ReputationTier::Untrusted);
    }

    #[test]
    fn three_consecutive_failures_blacklists() {
        let pm = metrics_from(&[false, false, false]);
        let s = compute(&pm);
        assert_eq!(s.tier, ReputationTier::Blacklisted);
        assert!(s.is_blacklisted());
    }

    #[test]
    fn success_after_two_failures_resets_blacklist_timer() {
        // fail, fail, succeed — consecutive_failures resets; should NOT blacklist
        let pm = metrics_from(&[false, false, true]);
        let s = compute(&pm);
        assert_ne!(s.tier, ReputationTier::Blacklisted);
    }

    #[test]
    fn five_successes_reaches_trusted() {
        let pm = metrics_from(&[true, true, true, true, true]);
        let s = compute(&pm);
        assert_eq!(s.tier, ReputationTier::Trusted);
    }

    #[test]
    fn ten_successes_reaches_promoted() {
        let pm = metrics_from(&[true, true, true, true, true, true, true, true, true, true]);
        let s = compute(&pm);
        assert_eq!(s.tier, ReputationTier::Promoted);
        assert!(s.is_promoted());
    }

    #[test]
    fn mixed_history_stays_emerging() {
        // 3 successes, 3 failures — not enough to trust
        let pm = metrics_from(&[true, false, true, false, true, false]);
        let s = compute(&pm);
        assert_eq!(s.tier, ReputationTier::Emerging);
    }

    #[test]
    fn trust_is_laplace_smoothed() {
        // 0 successes, 0 runs → 0.5
        let pm = PatternMetrics::default();
        let s = compute(&pm);
        assert!((s.trust - 0.5).abs() < 1e-9);
    }

    #[test]
    fn blacklist_check_precedes_trust_calculation() {
        // Even if somehow 50% success rate, 3 consecutive failures = blacklisted
        let pm = metrics_from(&[true, true, true, false, false, false]);
        let s = compute(&pm);
        assert_eq!(s.tier, ReputationTier::Blacklisted);
    }
}
