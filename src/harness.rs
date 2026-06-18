use crate::backends::InferenceEngine;
use crate::extractor;
use crate::soul;
use crate::types::{
    Config, HarnessResult, LogicReport, RawInput, Soul, TelemetryResult, TraceEntry,
};
use crate::verifier;
use anyhow::{anyhow, Result};

pub struct Harness<'e> {
    soul: Soul,
    engine: &'e dyn InferenceEngine,
    config: &'e Config,
}

impl<'e> Harness<'e> {
    pub fn new(soul: Soul, engine: &'e dyn InferenceEngine, config: &'e Config) -> Self {
        Self {
            soul,
            engine,
            config,
        }
    }

    /// Two-stage pipeline:
    /// 1. Propose — logic node produces TelemetryResult
    /// 2. Verify  — deterministic checks ± optional LLM verifier pass
    pub async fn analyze(&self, input: &str) -> Result<HarnessResult> {
        let mut trace: Vec<TraceEntry> = vec![];

        let (telemetry, propose_entry) = self.run_propose(input).await?;
        trace.push(propose_entry);

        let (verification, verify_traces) = verifier::verify(
            input,
            &telemetry,
            &self.soul,
            self.engine,
            &self.config.verify_mode,
        )
        .await;
        trace.extend(verify_traces);

        Ok(HarnessResult {
            telemetry,
            verification,
            trace,
        })
    }

    // -----------------------------------------------------------------------
    // Stage 1 — propose
    // -----------------------------------------------------------------------

    async fn run_propose(&self, input: &str) -> Result<(TelemetryResult, TraceEntry)> {
        let report = self.run_logic_node(RawInput(input.to_string())).await?;
        let telemetry: TelemetryResult = extractor::extract(&report.analytical_matrix)?;

        let entry = TraceEntry {
            stage: "propose".into(),
            claim: format!(
                "manipulation_risk={} emotion={} intensity={:.2}",
                telemetry.intent_matrix.manipulation_risk,
                telemetry.affective_telemetry.primary_emotion,
                telemetry.affective_telemetry.emotional_intensity,
            ),
            evidence: Some(truncate(input, 120)),
            passed: true,
            note: None,
        };

        Ok((telemetry, entry))
    }

    async fn run_logic_node(&self, input: RawInput) -> Result<LogicReport> {
        let payload = soul::wrap_payload(&input.0);

        let raw_response = self
            .engine
            .generate(&self.soul.logic_system_prompt, &payload)
            .await
            .map_err(|e| anyhow!("inference backend error: {}", e))?;

        if raw_response.trim().is_empty() {
            return Err(anyhow!("model returned an empty response"));
        }

        Ok(LogicReport {
            analytical_matrix: raw_response,
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
