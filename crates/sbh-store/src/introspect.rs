//! Offline meta-cognition over the harness's own history (advanced tier **G**).
//!
//! `introspect` reads the append-only stores this crate already writes — the
//! [`calibration`](crate::calibration) feature/feedback log and the
//! [`session_log`](crate::session_log) escalation log — and surfaces *recurring
//! failure patterns* plus **advisory** suggestions for weights and prompts.
//!
//! Three hard constraints, by design:
//!   1. **Offline & deterministic** — no LLM, no network, pure functions over the
//!      two stores. Same inputs always yield the same report.
//!   2. **Advisory only** — it proposes reviewable diffs; it never edits weights,
//!      the soul, or any config. Runtime self-modification is a non-goal.
//!   3. **Privacy-preserving** — the stores hold only FNV fingerprints, never raw
//!      input, so clustering keys off structured features (which checks fired,
//!      confidence band, fingerprint), not text. Suggestions say where to look.
//!
//! A *misread* is a run whose verdict a human later labelled incorrect via
//! `sbh feedback --misread`. The store does not record misread direction (false
//! positive vs false negative), so reports speak of "incorrect verdicts", not
//! "missed attacks".

use crate::calibration::{tune_weights, CalibrationEntry, CheckAdvice};
use crate::session_log::SessionLogEntry;
use std::collections::HashMap;

/// Coarse confidence band a run's predicted confidence fell in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Low,
    Mid,
    High,
}

impl Band {
    pub fn of(confidence: f32) -> Band {
        if confidence < 0.4 {
            Band::Low
        } else if confidence < 0.7 {
            Band::Mid
        } else {
            Band::High
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Band::Low => "low",
            Band::Mid => "mid",
            Band::High => "high",
        }
    }
}

/// What kind of blind spot a failure cluster represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    /// No deterministic check fired, yet the verdict was wrong — the pattern is
    /// invisible to the current checks (a coverage gap → prompt/soul territory).
    UnderDetection,
    /// The injection-fingerprint heuristic fired but the verdict was still wrong
    /// — the heuristic may be misfiring on this shape.
    FingerprintMisfire,
    /// Specific checks fired together but the verdict was wrong — candidate
    /// over-firing checks (→ weight territory).
    ChecksFiredButWrong,
}

impl Archetype {
    pub fn as_str(self) -> &'static str {
        match self {
            Archetype::UnderDetection => "under-detection",
            Archetype::FingerprintMisfire => "fingerprint-misfire",
            Archetype::ChecksFiredButWrong => "checks-fired-but-wrong",
        }
    }
}

/// A recurring failure signature across misread runs.
#[derive(Debug, Clone)]
pub struct FailureCluster {
    /// The set of checks that fired (sorted; empty = no check fired).
    pub fired_checks: Vec<String>,
    /// Whether the injection-fingerprint heuristic fired on these runs.
    pub injection_fingerprint: bool,
    /// The most common confidence band within the cluster.
    pub modal_band: Band,
    /// Number of misread runs sharing this signature.
    pub count: usize,
    /// Fraction of all misreads this cluster accounts for (0..1).
    pub share_of_misreads: f32,
    pub archetype: Archetype,
    /// Advisory, human-readable next step. Never applied automatically.
    pub suggestion: String,
}

/// Descriptive summary of the multi-turn escalation log.
#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    pub escalations: usize,
    /// Mean turn count at which escalations fired.
    pub mean_turns_to_escalation: f64,
    /// Escalations that jumped straight to "high" from a non-high prior turn
    /// (a slow-boil that wasn't actually slow — worth a rule/prompt look).
    pub jump_to_high: usize,
    /// Most common risk trajectories (rendered `low→low→high`), most frequent first.
    pub common_trajectories: Vec<(String, usize)>,
}

/// The full introspection report.
#[derive(Debug, Clone)]
pub struct IntrospectReport {
    pub calibration_entries: usize,
    pub labeled: usize,
    pub correct: usize,
    pub misreads: usize,
    pub clusters: Vec<FailureCluster>,
    /// Per-check reliability from [`tune_weights`], most-suspect first.
    pub check_advice: Vec<CheckAdvice>,
    pub session: SessionSummary,
}

/// Run the full offline introspection. `min_cluster` is the minimum number of
/// misreads a signature needs before it is reported as a cluster (data-hungry —
/// small stores yield little).
pub fn introspect(
    cal: &[CalibrationEntry],
    sessions: &[SessionLogEntry],
    min_cluster: usize,
) -> IntrospectReport {
    let min_cluster = min_cluster.max(1);

    // Join labels (arrive as separate feedback rows) to featureful runs by
    // fingerprint — mirrors calibration::tune_weights / labeled_samples.
    let mut labels: HashMap<&str, bool> = HashMap::new();
    for e in cal {
        if let Some(l) = e.label {
            labels.insert(e.input_fingerprint.as_str(), l);
        }
    }
    let mut correct = 0usize;
    let mut misread_runs: Vec<&CalibrationEntry> = Vec::new();
    for e in cal {
        if e.label.is_some() {
            continue; // label rows carry no features
        }
        match labels.get(e.input_fingerprint.as_str()) {
            Some(true) => correct += 1,
            Some(false) => misread_runs.push(e),
            None => {}
        }
    }
    let misreads = misread_runs.len();

    let clusters = cluster_failures(&misread_runs, min_cluster);
    let check_advice = tune_weights(cal);
    let session = summarize_sessions(sessions);

    IntrospectReport {
        calibration_entries: cal.iter().filter(|e| e.label.is_none()).count(),
        labeled: labels.len(),
        correct,
        misreads,
        clusters,
        check_advice,
        session,
    }
}

/// Signature key for grouping misreads: the sorted fired-check set plus whether
/// the injection fingerprint fired. Confidence band is summarised, not keyed, so
/// near-identical failures don't fragment across bands.
fn cluster_failures(misreads: &[&CalibrationEntry], min_cluster: usize) -> Vec<FailureCluster> {
    struct Acc {
        fired: Vec<String>,
        fingerprint: bool,
        bands: HashMap<&'static str, usize>,
        count: usize,
    }
    let mut groups: HashMap<(Vec<String>, bool), Acc> = HashMap::new();

    for e in misreads {
        let mut fired = e.features.fired_checks.clone();
        fired.sort();
        fired.dedup();
        let fp = e.features.injection_fingerprint;
        let band = Band::of(e.features.raw_confidence).as_str();
        let acc = groups.entry((fired.clone(), fp)).or_insert_with(|| Acc {
            fired,
            fingerprint: fp,
            bands: HashMap::new(),
            count: 0,
        });
        acc.count += 1;
        *acc.bands.entry(band).or_insert(0) += 1;
    }

    let total = misreads.len().max(1) as f32;
    let mut clusters: Vec<FailureCluster> = groups
        .into_values()
        .filter(|a| a.count >= min_cluster)
        .map(|a| {
            let modal_band = modal_band(&a.bands);
            let archetype = archetype_of(&a.fired, a.fingerprint);
            let suggestion = suggest(&a.fired, archetype, modal_band, a.count);
            FailureCluster {
                fired_checks: a.fired,
                injection_fingerprint: a.fingerprint,
                modal_band,
                count: a.count,
                share_of_misreads: a.count as f32 / total,
                archetype,
                suggestion,
            }
        })
        .collect();

    // Largest clusters first; ties broken deterministically by signature.
    clusters.sort_by(|x, y| {
        y.count
            .cmp(&x.count)
            .then_with(|| x.fired_checks.cmp(&y.fired_checks))
            .then_with(|| x.injection_fingerprint.cmp(&y.injection_fingerprint))
    });
    clusters
}

fn modal_band(bands: &HashMap<&'static str, usize>) -> Band {
    // Deterministic: highest count, ties broken by band order low<mid<high.
    let order = |b: &str| match b {
        "low" => 0,
        "mid" => 1,
        _ => 2,
    };
    let mut best = ("mid", 0usize);
    for (b, &c) in bands {
        if c > best.1 || (c == best.1 && order(b) < order(best.0)) {
            best = (b, c);
        }
    }
    match best.0 {
        "low" => Band::Low,
        "high" => Band::High,
        _ => Band::Mid,
    }
}

fn archetype_of(fired: &[String], fingerprint: bool) -> Archetype {
    if fired.is_empty() {
        Archetype::UnderDetection
    } else if fingerprint {
        Archetype::FingerprintMisfire
    } else {
        Archetype::ChecksFiredButWrong
    }
}

fn suggest(fired: &[String], archetype: Archetype, band: Band, count: usize) -> String {
    match archetype {
        Archetype::UnderDetection => format!(
            "{count} incorrect verdict(s) slipped past every deterministic check (confidence mostly {}). \
             This shape isn't covered by the current checks — consider strengthening subtextual-intent \
             extraction in the soul or adding a context pack. Pull these fingerprints from the audit/session \
             logs to inspect the inputs (raw text isn't stored here).",
            band.as_str()
        ),
        Archetype::FingerprintMisfire => format!(
            "{count} incorrect verdict(s) where the injection-fingerprint heuristic fired \
             (tone+urgency vs low manipulation_risk). Review whether the fingerprint over-triggers on \
             this shape, or whether the proposer's risk label was wrong."
        ),
        Archetype::ChecksFiredButWrong => format!(
            "{count} incorrect verdict(s) with checks [{}] firing together (confidence mostly {}). \
             Candidate over-firing checks — see the weight advice below before changing any weight.",
            fired.join(", "),
            band.as_str()
        ),
    }
}

fn summarize_sessions(sessions: &[SessionLogEntry]) -> SessionSummary {
    if sessions.is_empty() {
        return SessionSummary::default();
    }
    let escalations = sessions.len();
    let mean_turns_to_escalation =
        sessions.iter().map(|s| s.turn_count as f64).sum::<f64>() / escalations as f64;

    let mut jump_to_high = 0usize;
    let mut traj_counts: HashMap<String, usize> = HashMap::new();
    for s in sessions {
        if is_jump_to_high(&s.risk_trajectory) {
            jump_to_high += 1;
        }
        *traj_counts.entry(s.risk_trajectory.join("→")).or_insert(0) += 1;
    }
    let mut common_trajectories: Vec<(String, usize)> = traj_counts.into_iter().collect();
    common_trajectories.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    common_trajectories.truncate(5);

    SessionSummary {
        escalations,
        mean_turns_to_escalation,
        jump_to_high,
        common_trajectories,
    }
}

/// True when the trajectory ends "high" and the immediately preceding turn was
/// "low" — an abrupt two-level jump that skipped "medium", rather than a gradual
/// low→medium→high slow-boil.
fn is_jump_to_high(traj: &[String]) -> bool {
    if traj.last().map(String::as_str) != Some("high") {
        return false;
    }
    traj.get(traj.len().wrapping_sub(2)).map(String::as_str) == Some("low")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::{CalibrationEntry, CalibrationFeatures};

    fn feat(fp: &str, checks: &[&str], conf: f32, fingerprint: bool) -> CalibrationEntry {
        CalibrationEntry {
            timestamp: "t".into(),
            input_fingerprint: fp.into(),
            features: CalibrationFeatures {
                fired_checks: checks.iter().map(|s| s.to_string()).collect(),
                injection_fingerprint: fingerprint,
                raw_confidence: conf,
                ..Default::default()
            },
            predicted_confidence: conf,
            label: None,
        }
    }
    fn label(fp: &str, correct: bool) -> CalibrationEntry {
        crate::calibration::label_entry(fp, correct)
    }

    #[test]
    fn band_boundaries() {
        assert_eq!(Band::of(0.0), Band::Low);
        assert_eq!(Band::of(0.39), Band::Low);
        assert_eq!(Band::of(0.4), Band::Mid);
        assert_eq!(Band::of(0.69), Band::Mid);
        assert_eq!(Band::of(0.7), Band::High);
        assert_eq!(Band::of(1.0), Band::High);
    }

    #[test]
    fn counts_correct_and_misreads() {
        let cal = vec![
            feat("a", &[], 0.9, false),
            label("a", true),
            feat("b", &[], 0.8, false),
            label("b", false),
        ];
        let r = introspect(&cal, &[], 1);
        assert_eq!(r.correct, 1);
        assert_eq!(r.misreads, 1);
        assert_eq!(r.labeled, 2);
    }

    #[test]
    fn under_detection_cluster_is_flagged() {
        // Three misreads that fired no checks → an under-detection cluster.
        let mut cal = vec![];
        for i in 0..3 {
            let fp = format!("u{i}");
            cal.push(feat(&fp, &[], 0.85, false));
            cal.push(label(&fp, false));
        }
        let r = introspect(&cal, &[], 3);
        assert_eq!(r.clusters.len(), 1);
        let c = &r.clusters[0];
        assert_eq!(c.archetype, Archetype::UnderDetection);
        assert_eq!(c.count, 3);
        assert!(c.fired_checks.is_empty());
        assert_eq!(c.modal_band, Band::High);
        assert!((c.share_of_misreads - 1.0).abs() < 1e-6);
    }

    #[test]
    fn min_cluster_filters_small_signatures() {
        let mut cal = vec![];
        for i in 0..2 {
            let fp = format!("u{i}");
            cal.push(feat(&fp, &[], 0.85, false));
            cal.push(label(&fp, false));
        }
        // 2 misreads, threshold 3 → no cluster reported.
        let r = introspect(&cal, &[], 3);
        assert!(r.clusters.is_empty());
        assert_eq!(r.misreads, 2);
    }

    #[test]
    fn checks_fired_but_wrong_archetype() {
        let mut cal = vec![];
        for i in 0..3 {
            let fp = format!("c{i}");
            cal.push(feat(
                &fp,
                &["adversarial tone vs manipulation-risk"],
                0.5,
                false,
            ));
            cal.push(label(&fp, false));
        }
        let r = introspect(&cal, &[], 3);
        assert_eq!(r.clusters.len(), 1);
        assert_eq!(r.clusters[0].archetype, Archetype::ChecksFiredButWrong);
        assert_eq!(r.clusters[0].modal_band, Band::Mid);
    }

    #[test]
    fn fingerprint_misfire_archetype() {
        let mut cal = vec![];
        for i in 0..3 {
            let fp = format!("f{i}");
            cal.push(feat(
                &fp,
                &["adversarial tone vs manipulation-risk"],
                0.3,
                true,
            ));
            cal.push(label(&fp, false));
        }
        let r = introspect(&cal, &[], 3);
        assert_eq!(r.clusters[0].archetype, Archetype::FingerprintMisfire);
    }

    #[test]
    fn session_summary_and_jump_detection() {
        use crate::session_log::SessionLogEntry;
        use std::net::{IpAddr, Ipv4Addr};
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let slow = SessionLogEntry::new(
            "s1".into(),
            3,
            vec!["low".into(), "medium".into(), "high".into()],
            0.5,
            &ip,
            "x",
        );
        let jump = SessionLogEntry::new(
            "s2".into(),
            3,
            vec!["low".into(), "low".into(), "high".into()],
            0.0,
            &ip,
            "y",
        );
        let s = summarize_sessions(&[slow, jump]);
        assert_eq!(s.escalations, 2);
        assert!((s.mean_turns_to_escalation - 3.0).abs() < 1e-9);
        // "low→low→high" is a jump (prev turn low); "low→medium→high" is not.
        assert_eq!(s.jump_to_high, 1);
        assert_eq!(s.common_trajectories.len(), 2);
    }

    #[test]
    fn empty_stores_are_safe() {
        let r = introspect(&[], &[], 3);
        assert_eq!(r.misreads, 0);
        assert!(r.clusters.is_empty());
        assert_eq!(r.session.escalations, 0);
    }

    #[test]
    fn deterministic_output() {
        let mut cal = vec![];
        for i in 0..4 {
            let fp = format!("u{i}");
            cal.push(feat(&fp, &[], 0.85, false));
            cal.push(label(&fp, false));
        }
        let a = introspect(&cal, &[], 2);
        let b = introspect(&cal, &[], 2);
        assert_eq!(a.clusters.len(), b.clusters.len());
        assert_eq!(a.clusters[0].count, b.clusters[0].count);
        assert_eq!(a.clusters[0].suggestion, b.clusters[0].suggestion);
    }
}
