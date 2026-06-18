use crate::backends::InferenceEngine;
use crate::extractor;
use crate::soul;
use crate::types::{Soul, TelemetryResult, TraceEntry, VerificationReport, VerifyMode};
use serde::Deserialize;

const STOP_AND_ASK_THRESHOLD: f32 = 0.4;

type CheckFn = Box<dyn Fn(&TelemetryResult) -> Option<String>>;

/// Schema for the LLM verifier's JSON output.
#[derive(Deserialize, Default)]
struct VerifierLLMOutput {
    supported: bool,
    unsupported_claims: Vec<String>,
    assumptions: Vec<String>,
    unresolved: Vec<String>,
    confidence: f32,
}

/// Run the full verification stage. Returns a (report, traces) pair.
/// Deterministic checks always run unless mode is None.
/// LLM pass only runs when mode is Llm.
pub async fn verify(
    input: &str,
    telemetry: &TelemetryResult,
    soul: &Soul,
    engine: &dyn InferenceEngine,
    mode: &VerifyMode,
) -> (VerificationReport, Vec<TraceEntry>) {
    let mut traces = vec![];

    let (consistency_flags, det_traces) = match mode {
        VerifyMode::None => (vec![], vec![]),
        _ => check_consistency(telemetry),
    };
    traces.extend(det_traces);

    let (unsupported_claims, assumptions, unresolved, llm_confidence) = match mode {
        VerifyMode::Llm => match run_llm_verify(input, telemetry, soul, engine).await {
            Ok((out, t)) => {
                traces.push(t);
                (
                    out.unsupported_claims,
                    out.assumptions,
                    out.unresolved,
                    Some(out.confidence),
                )
            }
            Err(e) => {
                traces.push(TraceEntry {
                    stage: "verify-llm".into(),
                    claim: "LLM verifier call failed".into(),
                    evidence: None,
                    passed: false,
                    note: Some(e.to_string()),
                });
                (vec![], vec![], vec![format!("verifier error: {e}")], None)
            }
        },
        _ => (vec![], vec![], vec![], None),
    };

    let confidence = derive_confidence(telemetry, &consistency_flags, llm_confidence);
    let stop_and_ask = confidence < STOP_AND_ASK_THRESHOLD || consistency_flags.len() >= 3;
    let passed = consistency_flags.is_empty() && unsupported_claims.is_empty();

    let report = VerificationReport {
        passed,
        consistency_flags,
        unsupported_claims,
        assumptions,
        unresolved,
        confidence,
        stop_and_ask,
    };

    (report, traces)
}

// ---------------------------------------------------------------------------
// Deterministic consistency checks
// ---------------------------------------------------------------------------

fn check_consistency(t: &TelemetryResult) -> (Vec<String>, Vec<TraceEntry>) {
    let mut flags = vec![];
    let mut traces = vec![];

    let checks: &[(&str, CheckFn)] = &[
        (
            "emotion-intensity vs manipulation-risk",
            Box::new(|t| {
                let hostile = ["anger", "fear", "contempt", "hostility"];
                if t.affective_telemetry.emotional_intensity >= 0.7
                    && t.intent_matrix.manipulation_risk == "low"
                    && hostile.contains(
                        &t.affective_telemetry
                            .primary_emotion
                            .to_lowercase()
                            .as_str(),
                    )
                {
                    Some(format!(
                        "high emotional_intensity ({:.2}) with hostile emotion '{}' but manipulation_risk=low",
                        t.affective_telemetry.emotional_intensity,
                        t.affective_telemetry.primary_emotion
                    ))
                } else {
                    None
                }
            }),
        ),
        (
            "adversarial tone vs manipulation-risk",
            Box::new(|t| {
                let adversarial = ["adversarial", "manipulative", "coercive", "threatening"];
                let found: Vec<&str> = t
                    .affective_telemetry
                    .structural_tone
                    .iter()
                    .filter(|s| adversarial.contains(&s.to_lowercase().as_str()))
                    .map(|s| s.as_str())
                    .collect();
                if !found.is_empty() && t.intent_matrix.manipulation_risk == "low" {
                    Some(format!(
                        "structural_tone {:?} conflicts with manipulation_risk=low",
                        found
                    ))
                } else {
                    None
                }
            }),
        ),
        (
            "urgency vs manipulation-risk",
            Box::new(|t| {
                if t.cognitive_state.urgency_vector >= 0.7
                    && t.intent_matrix.manipulation_risk == "low"
                {
                    Some(format!(
                        "high urgency_vector ({:.2}) with manipulation_risk=low — urgency may be manufactured",
                        t.cognitive_state.urgency_vector
                    ))
                } else {
                    None
                }
            }),
        ),
        (
            "input coherence",
            Box::new(|t| {
                if t.cognitive_state.coherence_rating < 0.3 {
                    Some(format!(
                        "coherence_rating={:.2} — input may be too incoherent for reliable analysis",
                        t.cognitive_state.coherence_rating
                    ))
                } else {
                    None
                }
            }),
        ),
    ];

    for (name, check) in checks {
        match check(t) {
            Some(flag) => {
                flags.push(flag.clone());
                traces.push(TraceEntry {
                    stage: "verify-deterministic".into(),
                    claim: name.to_string(),
                    evidence: None,
                    passed: false,
                    note: Some(flag),
                });
            }
            None => {
                traces.push(TraceEntry {
                    stage: "verify-deterministic".into(),
                    claim: name.to_string(),
                    evidence: None,
                    passed: true,
                    note: None,
                });
            }
        }
    }

    (flags, traces)
}

// ---------------------------------------------------------------------------
// LLM verifier pass
// ---------------------------------------------------------------------------

async fn run_llm_verify(
    input: &str,
    telemetry: &TelemetryResult,
    soul: &Soul,
    engine: &dyn InferenceEngine,
) -> anyhow::Result<(VerifierLLMOutput, TraceEntry)> {
    if soul.verifier_system_prompt.is_empty() {
        return Err(anyhow::anyhow!(
            "verifier soul prompt is empty — add a [VERIFIER_SYSTEM_PROMPT] section to soul.md"
        ));
    }

    let proposed_json = serde_json::to_string_pretty(telemetry)?;
    let payload = soul::wrap_verifier_payload(input, &proposed_json);

    let raw = engine
        .generate(&soul.verifier_system_prompt, &payload)
        .await
        .map_err(|e| anyhow::anyhow!("verifier inference error: {e}"))?;

    let out: VerifierLLMOutput = extractor::extract(&raw).unwrap_or_default();

    let note = if out.unsupported_claims.is_empty() {
        None
    } else {
        Some(out.unsupported_claims.join("; "))
    };

    let trace = TraceEntry {
        stage: "verify-llm".into(),
        claim: format!("confidence={:.2}", out.confidence),
        evidence: if out.unsupported_claims.is_empty() {
            None
        } else {
            Some(format!("unsupported: {:?}", out.unsupported_claims))
        },
        passed: out.supported && out.unsupported_claims.is_empty(),
        note,
    };

    Ok((out, trace))
}

// ---------------------------------------------------------------------------
// Confidence derivation
// ---------------------------------------------------------------------------

fn derive_confidence(t: &TelemetryResult, flags: &[String], llm_confidence: Option<f32>) -> f32 {
    let base = t.cognitive_state.coherence_rating;
    let penalty = (flags.len() as f32) * 0.15;
    let score = (base - penalty).clamp(0.0, 1.0);
    match llm_confidence {
        Some(llm) => ((score + llm) / 2.0).clamp(0.0, 1.0),
        None => score,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AfferentTelemetry, CognitiveState, IntentMatrix, TelemetryResult};

    fn make_telemetry(
        emotion: &str,
        intensity: f32,
        tone: Vec<&str>,
        risk: &str,
        urgency: f32,
        coherence: f32,
    ) -> TelemetryResult {
        TelemetryResult {
            affective_telemetry: AfferentTelemetry {
                primary_emotion: emotion.into(),
                emotional_intensity: intensity,
                structural_tone: tone.into_iter().map(String::from).collect(),
            },
            intent_matrix: IntentMatrix {
                stated_objective: "test objective".into(),
                subtextual_motive: "test motive".into(),
                manipulation_risk: risk.into(),
            },
            cognitive_state: CognitiveState {
                urgency_vector: urgency,
                coherence_rating: coherence,
            },
        }
    }

    // --- Adversarial: flags that should fire ---

    #[test]
    fn flags_hostile_high_intensity_low_risk() {
        let t = make_telemetry("anger", 0.85, vec!["demanding"], "low", 0.3, 0.9);
        let (flags, _) = check_consistency(&t);
        assert!(
            flags.iter().any(|f| f.contains("emotional_intensity")),
            "should flag hostile emotion + high intensity vs low risk"
        );
    }

    #[test]
    fn flags_adversarial_tone_low_risk() {
        let t = make_telemetry(
            "neutral",
            0.2,
            vec!["adversarial", "coercive"],
            "low",
            0.1,
            0.9,
        );
        let (flags, _) = check_consistency(&t);
        assert!(
            flags.iter().any(|f| f.contains("structural_tone")),
            "should flag adversarial tone vs low risk"
        );
    }

    #[test]
    fn flags_high_urgency_low_risk() {
        let t = make_telemetry("neutral", 0.2, vec!["cooperative"], "low", 0.8, 0.9);
        let (flags, _) = check_consistency(&t);
        assert!(
            flags.iter().any(|f| f.contains("urgency_vector")),
            "should flag high urgency vs low risk"
        );
    }

    #[test]
    fn flags_low_coherence() {
        let t = make_telemetry("neutral", 0.2, vec!["incoherent"], "low", 0.1, 0.2);
        let (flags, _) = check_consistency(&t);
        assert!(
            flags.iter().any(|f| f.contains("coherence_rating")),
            "should flag low coherence"
        );
    }

    // --- Clean inputs: should pass all checks ---

    #[test]
    fn clean_benign_passes_all_checks() {
        let t = make_telemetry(
            "neutral",
            0.05,
            vec!["cooperative", "inquisitive"],
            "low",
            0.05,
            0.98,
        );
        let (flags, traces) = check_consistency(&t);
        assert!(
            flags.is_empty(),
            "clean benign input should pass all checks"
        );
        assert!(
            traces.iter().all(|t| t.passed),
            "all traces should be passed"
        );
    }

    #[test]
    fn high_risk_high_intensity_passes() {
        // adversarial + high risk is internally consistent — should not flag
        let t = make_telemetry(
            "anger",
            0.9,
            vec!["adversarial", "threatening"],
            "high",
            0.8,
            0.85,
        );
        let (flags, _) = check_consistency(&t);
        assert!(
            !flags.iter().any(|f| f.contains("structural_tone")),
            "adversarial tone with high risk should not flag"
        );
    }

    // --- Confidence derivation ---

    #[test]
    fn confidence_equals_coherence_when_no_flags() {
        let t = make_telemetry("neutral", 0.1, vec!["analytical"], "low", 0.0, 0.95);
        let (flags, _) = check_consistency(&t);
        let confidence = derive_confidence(&t, &flags, None);
        assert!((confidence - 0.95).abs() < 0.01);
    }

    #[test]
    fn confidence_penalized_per_flag() {
        let t = make_telemetry("anger", 0.85, vec!["adversarial"], "low", 0.8, 0.9);
        let (flags, _) = check_consistency(&t);
        let confidence = derive_confidence(&t, &flags, None);
        assert!(confidence < 0.9, "each flag should reduce confidence");
    }

    #[test]
    fn stop_and_ask_triggers_at_threshold() {
        // 3 flags on a coherent input → stop_and_ask regardless of confidence
        let flags: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let t = make_telemetry("neutral", 0.5, vec![], "medium", 0.5, 0.9);
        let confidence = derive_confidence(&t, &flags, None);
        let stop = confidence < STOP_AND_ASK_THRESHOLD || flags.len() >= 3;
        assert!(stop, "3 flags should always trigger stop_and_ask");
    }

    // --- Adversarial inputs ---

    #[test]
    fn contradictory_risk_vs_tone_flagged() {
        // manipulation_risk=high + cooperative tone would be fine.
        // But adversarial tone + low risk should flag.
        let t = make_telemetry("enthusiasm", 0.3, vec!["manipulative"], "low", 0.2, 0.85);
        let (flags, _) = check_consistency(&t);
        assert!(
            !flags.is_empty(),
            "manipulative tone vs low risk should flag"
        );
    }

    #[test]
    fn missing_context_low_coherence_stops() {
        // Simulates a chaotic / fragment input that barely parsed
        let t = make_telemetry("confusion", 0.4, vec!["scattered"], "medium", 0.3, 0.18);
        let (flags, _) = check_consistency(&t);
        let confidence = derive_confidence(&t, &flags, None);
        let stop = confidence < STOP_AND_ASK_THRESHOLD || flags.len() >= 3;
        assert!(stop, "low coherence should trigger stop_and_ask");
    }
}
