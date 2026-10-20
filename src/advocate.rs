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
//! `verifier::run_reconcile` template: fixed Fair Witness identity and one fresh
//! slice, parsed strictly into verified byte spans and closed enum codes.

use sbh_core::witness_schema::{parse_report, WatchSlice, WatchVerdict};
use sbh_safety::fair_witness;

use crate::backends::InferenceEngine;
use crate::capability::CapabilityRequest;
use crate::types::{AdvocateMode, AdvocateReport, Risk, Soul, TelemetryResult, ToolRisk};

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

/// Run the adversarial pass. Errors (backend or strict-parse failure) propagate to the caller, which treats them as **advisory** — a
/// transient advocate failure must not fail closed (that would be a DoS lever on
/// a flaky backend), unlike the deterministic Formal stage.
pub async fn run(
    input: &str,
    telemetry: &TelemetryResult,
    _soul: &Soul,
    engine: &dyn InferenceEngine,
    gate_reason: Vec<String>,
) -> anyhow::Result<AdvocateReport> {
    let slice = WatchSlice::new(vec![input.into(), serde_json::to_string(telemetry)?]);
    let raw = engine
        .generate(
            &fair_witness::system_prompt(),
            &fair_witness::user_payload(&slice),
        )
        .await
        .map_err(|e| anyhow::anyhow!("advocate inference error: {e}"))?;
    let witness = parse_report(&raw, &slice)
        .map_err(|_| anyhow::anyhow!("advocate parse failed: strict witness schema rejected"))?;
    let dissented = witness.effective_verdict() == WatchVerdict::Halt;
    Ok(AdvocateReport {
        witness,
        dissented,
        gate_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AfferentTelemetry, CognitiveState, IntentMatrix};

    struct Capture(std::sync::Mutex<Vec<(String, String)>>);
    #[async_trait::async_trait]
    impl InferenceEngine for Capture {
        async fn generate(&self, system: &str, payload: &str) -> Result<String, String> {
            self.0.lock().unwrap().push((system.into(), payload.into()));
            Ok(r#"{"verdict":"HALT","findings":[{"code":"OBSERVER_ADDRESSED","entry":0,"quote":"FIRST_ADVOCATE"}]}"#.into())
        }
    }

    #[tokio::test]
    async fn every_advocate_prompt_is_an_isolated_frame() {
        let engine = Capture(Default::default());
        let mut soul = crate::soul::load(None).unwrap();
        soul.advocate_system_prompt = "GENERATOR_OVERRIDE_SENTINEL".into();
        for input in ["FIRST_ADVOCATE", "SECOND_ADVOCATE"] {
            let report = run(
                input,
                &telem(Risk::Low),
                &soul,
                &engine,
                vec!["PRIVATE_GATE_REASON".into()],
            )
            .await
            .unwrap();
            if input == "FIRST_ADVOCATE" {
                assert_eq!(report.witness.findings[0].start, 0);
            } else {
                assert!(report.witness.findings.is_empty());
                assert_eq!(report.witness.rejected.len(), 1);
            }
        }
        let calls = engine.0.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (i, (system, payload)) in calls.iter().enumerate() {
            assert_eq!(system, &fair_witness::system_prompt());
            let entries: Vec<String> = serde_json::from_str(
                payload
                    .strip_prefix("<slice>\n")
                    .unwrap()
                    .strip_suffix("\n</slice>")
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                entries,
                vec![
                    ["FIRST_ADVOCATE", "SECOND_ADVOCATE"][i].to_string(),
                    serde_json::to_string(&telem(Risk::Low)).unwrap()
                ]
            );
            assert!(!payload.contains("PRIVATE_GATE_REASON"));
            assert!(!payload.contains("OBSERVER_ADDRESSED"));
        }
        assert!(!calls[1].1.contains("FIRST_ADVOCATE"));
    }

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
