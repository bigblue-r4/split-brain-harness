//! Devil's-Advocate / debate stage (phase E) — an adversarial third LLM pass
//! that argues the proposer's benign read is *wrong*: that the input is
//! manipulation. Its dissent can only ever **raise** caution on the final gate,
//! never lower it — so an attacker cannot steer the advocate into vouching for
//! their own input.
//!
//! Cost control: the advocate is gated. In `HighStakes` mode a **deterministic**
//! predicate (high manipulation risk, a risky tool surface, or a capability
//! request) decides whether the one extra call is worth making — so the advocate
//! never runs on obviously-benign traffic. The call itself reuses the
//! `verifier::run_reconcile` template: soul-wrapped system prompt + a structured
//! payload → `extractor::extract` into a typed opinion.

use serde::Deserialize;

use crate::backends::InferenceEngine;
use crate::capability::CapabilityRequest;
use crate::types::{AdvocateMode, AdvocateReport, Risk, Soul, TelemetryResult, ToolRisk};

/// Minimum advocate confidence in an "attack" verdict for its dissent to
/// escalate the gate. Below this the opinion is recorded but does not force
/// stop_and_ask — a hedged accusation should not by itself halt the pipeline.
pub const DISSENT_THRESHOLD: f32 = 0.6;

/// The advocate's raw JSON reply.
#[derive(Deserialize)]
struct AdvocateOutput {
    verdict: String,
    confidence: f32,
    #[serde(default)]
    argument: String,
}

/// Decide whether the advocate should run, and why. `None` = skip (no LLM call).
/// The reasons are recorded on the report for transparency.
pub fn gate(
    mode: &AdvocateMode,
    telemetry: &TelemetryResult,
    tool_risk: Option<&ToolRisk>,
    capability_request: Option<&CapabilityRequest>,
) -> Option<Vec<String>> {
    match mode {
        AdvocateMode::Off => None,
        AdvocateMode::Always => Some(vec!["advocate_mode=always".into()]),
        AdvocateMode::HighStakes => {
            let mut reasons = Vec::new();
            if telemetry.intent_matrix.manipulation_risk == Risk::High {
                reasons.push("manipulation_risk=high".into());
            }
            if let Some(tr) = tool_risk {
                if tr.code_execution {
                    reasons.push("surface:code_execution".into());
                }
                if tr.shell {
                    reasons.push("surface:shell".into());
                }
                if tr.network {
                    reasons.push("surface:network".into());
                }
            }
            if capability_request.is_some() {
                reasons.push("capability_request".into());
            }
            (!reasons.is_empty()).then_some(reasons)
        }
    }
}

/// Run the adversarial pass. Errors (backend failure, empty soul prompt, parse
/// failure) propagate to the caller, which treats them as **advisory** — a
/// transient advocate failure must not fail closed (that would be a DoS lever on
/// a flaky backend), unlike the deterministic Formal stage.
pub async fn run(
    input: &str,
    telemetry: &TelemetryResult,
    soul: &Soul,
    engine: &dyn InferenceEngine,
    gate_reason: Vec<String>,
) -> anyhow::Result<AdvocateReport> {
    if soul.advocate_system_prompt.is_empty() {
        anyhow::bail!(
            "advocate soul prompt is empty — add an [ADVOCATE_SYSTEM_PROMPT] section to soul.md"
        );
    }
    let telemetry_json = serde_json::to_string_pretty(telemetry)?;
    let payload = format!(
        "<original_input>\n{input}\n</original_input>\n\
         <proposer_telemetry>\n{telemetry_json}\n</proposer_telemetry>"
    );

    let raw = engine
        .generate(&soul.advocate_system_prompt, &payload)
        .await
        .map_err(|e| anyhow::anyhow!("advocate inference error: {e}"))?;

    let out: AdvocateOutput = crate::extractor::extract(&raw).map_err(|e| {
        let preview: String = raw.chars().take(200).collect();
        anyhow::anyhow!("advocate parse failed: {e}\n  raw (first 200 chars): {preview}")
    })?;

    let verdict = out.verdict.trim().to_lowercase();
    let confidence = out.confidence.clamp(0.0, 1.0);
    let dissented = verdict == "attack" && confidence >= DISSENT_THRESHOLD;

    Ok(AdvocateReport {
        verdict,
        confidence,
        argument: out.argument,
        dissented,
        gate_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AfferentTelemetry, CognitiveState, IntentMatrix};

    fn telem(risk: Risk) -> TelemetryResult {
        TelemetryResult {
            affective_telemetry: AfferentTelemetry {
                primary_emotion: "neutral".into(),
                emotional_intensity: 0.0,
                structural_tone: vec![],
            },
            intent_matrix: IntentMatrix {
                stated_objective: "do a thing".into(),
                subtextual_motive: String::new(),
                manipulation_risk: risk,
            },
            cognitive_state: CognitiveState {
                urgency_vector: 0.0,
                coherence_rating: 1.0,
            },
        }
    }

    fn net_surface() -> ToolRisk {
        ToolRisk {
            network: true,
            ..Default::default()
        }
    }

    #[test]
    fn off_never_gates() {
        assert!(gate(
            &AdvocateMode::Off,
            &telem(Risk::High),
            Some(&net_surface()),
            None
        )
        .is_none());
    }

    #[test]
    fn always_gates_unconditionally() {
        let r = gate(&AdvocateMode::Always, &telem(Risk::Low), None, None).unwrap();
        assert_eq!(r, vec!["advocate_mode=always"]);
    }

    #[test]
    fn high_stakes_skips_benign_low_risk() {
        assert!(gate(&AdvocateMode::HighStakes, &telem(Risk::Low), None, None).is_none());
    }

    #[test]
    fn high_stakes_fires_on_high_risk() {
        let r = gate(&AdvocateMode::HighStakes, &telem(Risk::High), None, None).unwrap();
        assert!(r.contains(&"manipulation_risk=high".to_string()));
    }

    #[test]
    fn high_stakes_fires_on_risky_surface() {
        let r = gate(
            &AdvocateMode::HighStakes,
            &telem(Risk::Low),
            Some(&net_surface()),
            None,
        )
        .unwrap();
        assert!(r.contains(&"surface:network".to_string()));
    }

    // Async run() paths are exercised end-to-end in harness integration tests
    // (MockEngine), which cover dissent escalation and the raise-only guardrail.
}
