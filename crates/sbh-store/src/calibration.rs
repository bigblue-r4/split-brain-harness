//! Confidence-calibration store (v1.5 scaffold).
//!
//! Append-only JSONL, mirroring `audit.rs` / `session_log.rs`. Every verification
//! logs its structured features and the raw confidence it produced. Once real
//! outcome labels are captured (human-in-the-loop, `sbh feedback`), an offline
//! `sbh calibrate` pass fits a 1-D **Platt-scaling** sigmoid so reported
//! confidence better matches observed correctness.
//!
//! Until a fitted `*.params.json` exists, confidence is passed through unchanged
//! (identity) — enabling the store is a no-op on behavior, by design. No raw input
//! is ever stored; entries carry only an FNV fingerprint (privacy parity with
//! `session_log.rs`).

use crate::audit::{fingerprint, iso_now};
use sbh_core::types::VerificationReport;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;

/// Minimum labeled samples required before a fit is attempted.
pub const MIN_SAMPLES: usize = 10;

/// Structured features logged per verification (for future fitting/inspection).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CalibrationFeatures {
    pub flag_density: f32,
    pub dimension_spread: usize,
    pub coherence: f32,
    pub injection_fingerprint: bool,
    pub raw_confidence: f32,
    /// IDs of the deterministic checks that fired (for HITL weight-tuning, D).
    #[serde(default)]
    pub fired_checks: Vec<String>,
}

/// One append-only calibration record.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CalibrationEntry {
    pub timestamp: String,
    /// FNV-1a-64 fingerprint of the input (raw input is never stored).
    pub input_fingerprint: String,
    pub features: CalibrationFeatures,
    /// The confidence the pipeline produced (the value we want to calibrate).
    pub predicted_confidence: f32,
    /// Outcome label, once known: `true` = the verdict was correct. Appended by
    /// `sbh feedback`; `None` on the initial featureful log line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<bool>,
}

/// Fitted Platt-scaling parameters: `calibrated = sigmoid(a * confidence + b)`.
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct PlattParams {
    pub a: f32,
    pub b: f32,
}

/// Build a featureful (unlabeled) entry from a verification report.
pub fn entry_from(input: &str, report: &VerificationReport) -> CalibrationEntry {
    CalibrationEntry {
        timestamp: iso_now(),
        input_fingerprint: fingerprint(input.as_bytes()),
        features: CalibrationFeatures {
            flag_density: report.disagreement.flag_density,
            dimension_spread: report.disagreement.dimension_spread,
            coherence: report.disagreement.adjusted_confidence, // structural base
            injection_fingerprint: report.disagreement.injection_fingerprint,
            raw_confidence: report.confidence,
            fired_checks: report.fired_checks.clone(),
        },
        predicted_confidence: report.confidence,
        label: None,
    }
}

/// A labeled feedback record referencing a prior run by fingerprint.
pub fn label_entry(input_fingerprint: &str, correct: bool) -> CalibrationEntry {
    CalibrationEntry {
        timestamp: iso_now(),
        input_fingerprint: input_fingerprint.to_string(),
        features: CalibrationFeatures::default(),
        predicted_confidence: 0.0,
        label: Some(correct),
    }
}

pub fn append(path: &str, entry: &CalibrationEntry) -> std::io::Result<()> {
    let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

pub fn read_all(path: &str) -> std::io::Result<Vec<CalibrationEntry>> {
    let raw = std::fs::read_to_string(path)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<CalibrationEntry>(l).ok())
        .collect())
}

/// Path of the fitted-parameters file that sits beside a calibration store.
pub fn params_path(store_path: &str) -> String {
    format!("{store_path}.params.json")
}

pub fn load_params(store_path: &str) -> Option<PlattParams> {
    let raw = std::fs::read_to_string(params_path(store_path)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_params(store_path: &str, params: &PlattParams) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(params).map_err(std::io::Error::other)?;
    std::fs::write(params_path(store_path), json)
}

/// Apply fitted calibration to a confidence value.
pub fn apply(params: &PlattParams, confidence: f32) -> f32 {
    let z = params.a * confidence + params.b;
    sigmoid(z).clamp(0.0, 1.0)
}

fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

/// Join featureful entries with their labels by fingerprint, yielding
/// `(predicted_confidence, label)` training pairs.
pub fn labeled_samples(entries: &[CalibrationEntry]) -> Vec<(f32, bool)> {
    // Latest predicted_confidence per fingerprint from the featureful lines.
    let mut pred: HashMap<&str, f32> = HashMap::new();
    for e in entries {
        if e.label.is_none() {
            pred.insert(e.input_fingerprint.as_str(), e.predicted_confidence);
        }
    }
    // Latest label per fingerprint.
    let mut labels: HashMap<&str, bool> = HashMap::new();
    for e in entries {
        if let Some(l) = e.label {
            labels.insert(e.input_fingerprint.as_str(), l);
        }
    }
    labels
        .into_iter()
        .filter_map(|(fp, y)| pred.get(fp).map(|&x| (x, y)))
        .collect()
}

/// Fit 1-D Platt scaling by gradient descent on log-loss. Returns `None` when
/// there are too few samples or the labels are single-class (nothing to fit).
pub fn fit_platt(samples: &[(f32, bool)]) -> Option<PlattParams> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let positives = samples.iter().filter(|(_, y)| *y).count();
    if positives == 0 || positives == samples.len() {
        return None; // single-class — a sigmoid can't be fit meaningfully
    }

    let n = samples.len() as f64;
    let (mut a, mut b) = (1.0_f64, 0.0_f64);
    let lr = 0.5;
    for _ in 0..5000 {
        let (mut ga, mut gb) = (0.0_f64, 0.0_f64);
        for &(x, y) in samples {
            let x = x as f64;
            let y = if y { 1.0 } else { 0.0 };
            let p = 1.0 / (1.0 + (-(a * x + b)).exp());
            ga += (p - y) * x;
            gb += p - y;
        }
        a -= lr * ga / n;
        b -= lr * gb / n;
    }
    Some(PlattParams {
        a: a as f32,
        b: b as f32,
    })
}

/// Advisory weight-tuning signal for one check (D). `correct_rate` is the
/// fraction of labeled entries where this check fired AND the verdict was correct.
#[derive(Debug, Clone)]
pub struct CheckAdvice {
    pub check: String,
    pub fired: usize,
    pub correct_when_fired: usize,
    pub correct_rate: f32,
    pub suggestion: &'static str,
}

/// Correlate each deterministic check's firing with feedback labels. A check that
/// fires often but on entries whose verdict was *wrong* is over-firing — advise
/// lowering its weight; one that fires on correct verdicts is pulling its weight.
/// **Advisory only** — the caller reviews before touching any weight.
pub fn tune_weights(entries: &[CalibrationEntry]) -> Vec<CheckAdvice> {
    use std::collections::HashMap;
    // Labels arrive as separate feedback rows; join by fingerprint.
    let mut labels: HashMap<&str, bool> = HashMap::new();
    for e in entries {
        if let Some(l) = e.label {
            labels.insert(e.input_fingerprint.as_str(), l);
        }
    }
    // Per check: (times fired on a labeled entry, times the verdict was correct).
    let mut stat: HashMap<String, (usize, usize)> = HashMap::new();
    for e in entries {
        if e.label.is_some() {
            continue; // featureful rows carry fired_checks; label rows don't
        }
        let Some(&correct) = labels.get(e.input_fingerprint.as_str()) else {
            continue;
        };
        for c in &e.features.fired_checks {
            let s = stat.entry(c.clone()).or_insert((0, 0));
            s.0 += 1;
            if correct {
                s.1 += 1;
            }
        }
    }
    let mut out: Vec<CheckAdvice> = stat
        .into_iter()
        .map(|(check, (fired, correct))| {
            let rate = if fired > 0 {
                correct as f32 / fired as f32
            } else {
                0.0
            };
            let suggestion = if fired < 3 {
                "insufficient data"
            } else if rate < 0.5 {
                "lower weight — over-fires"
            } else if rate > 0.8 {
                "raise weight — reliable"
            } else {
                "keep"
            };
            CheckAdvice {
                check,
                fired,
                correct_when_fired: correct,
                correct_rate: rate,
                suggestion,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.correct_rate
            .partial_cmp(&b.correct_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir
            .join(format!("sbh_cal_{}.jsonl", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);
        let e = label_entry("deadbeef", true);
        append(&path, &e).unwrap();
        let back = read_all(&path).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].label, Some(true));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn labeled_samples_joins_by_fingerprint() {
        let mut feat = CalibrationEntry {
            timestamp: "t".into(),
            input_fingerprint: "aaaa".into(),
            features: CalibrationFeatures::default(),
            predicted_confidence: 0.8,
            label: None,
        };
        feat.features.raw_confidence = 0.8;
        let lab = label_entry("aaaa", true);
        let orphan = label_entry("bbbb", false); // no featureful entry → dropped
        let samples = labeled_samples(&[feat, lab, orphan]);
        assert_eq!(samples.len(), 1);
        assert!((samples[0].0 - 0.8).abs() < 1e-6);
        assert!(samples[0].1);
    }

    #[test]
    fn tune_weights_distinguishes_overfiring_from_reliable() {
        let feat = |fp: &str, checks: &[&str]| CalibrationEntry {
            timestamp: "t".into(),
            input_fingerprint: fp.into(),
            features: CalibrationFeatures {
                fired_checks: checks.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            predicted_confidence: 0.5,
            label: None,
        };
        let mut entries = vec![];
        for i in 0..4 {
            // "noisy" fires but the verdict was always wrong
            entries.push(feat(&format!("n{i}"), &["noisy"]));
            entries.push(label_entry(&format!("n{i}"), false));
            // "good" fires and the verdict was always correct
            entries.push(feat(&format!("g{i}"), &["good"]));
            entries.push(label_entry(&format!("g{i}"), true));
        }
        let advice = tune_weights(&entries);
        let noisy = advice.iter().find(|a| a.check == "noisy").unwrap();
        let good = advice.iter().find(|a| a.check == "good").unwrap();
        assert_eq!(noisy.correct_rate, 0.0);
        assert!(noisy.suggestion.contains("lower"));
        assert_eq!(good.correct_rate, 1.0);
        assert!(good.suggestion.contains("raise"));
    }

    #[test]
    fn fit_returns_none_below_min_samples() {
        let samples = vec![(0.9, true), (0.1, false)];
        assert!(fit_platt(&samples).is_none());
    }

    #[test]
    fn fit_returns_none_single_class() {
        let samples: Vec<(f32, bool)> = (0..20).map(|i| (i as f32 / 20.0, true)).collect();
        assert!(fit_platt(&samples).is_none());
    }

    #[test]
    fn fit_learns_monotonic_mapping() {
        // High confidence → correct, low confidence → incorrect. A fitted model
        // must map a high score above a low score.
        let mut samples = vec![];
        for i in 0..40 {
            let x = i as f32 / 40.0;
            samples.push((x, x >= 0.5));
        }
        let params = fit_platt(&samples).expect("should fit");
        let hi = apply(&params, 0.9);
        let lo = apply(&params, 0.1);
        assert!(hi > lo, "calibrated high ({hi}) should exceed low ({lo})");
    }

    #[test]
    fn apply_with_identity_like_params_is_bounded() {
        let p = PlattParams { a: 1.0, b: 0.0 };
        let c = apply(&p, 0.5);
        assert!((0.0..=1.0).contains(&c));
    }
}
