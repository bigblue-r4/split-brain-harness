use crate::adaptor;
use crate::backends::InferenceEngine;
use crate::context_packs::ContextPack;
use crate::extractor;
use crate::types::{Config, HarnessResult, LogicReport, Soul, TelemetryResult, TraceEntry};
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
    /// 1. Propose — logic node (with context pack augmentation) produces TelemetryResult
    /// 2. Verify  — deterministic checks ± optional LLM verifier pass
    pub async fn analyze(&self, input: &str) -> Result<HarnessResult> {
        let mut trace: Vec<TraceEntry> = vec![];

        let (telemetry, propose_entries) = self.run_propose(input).await?;
        trace.extend(propose_entries);

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

    async fn run_propose(&self, input: &str) -> Result<(TelemetryResult, Vec<TraceEntry>)> {
        let active_packs = adaptor::select_packs(input);
        let mut entries: Vec<TraceEntry> = vec![];

        if !active_packs.is_empty() {
            let names: Vec<&str> = active_packs.iter().map(|p| p.name).collect();
            entries.push(TraceEntry {
                stage: "context_injection".into(),
                claim: format!(
                    "{} context pack(s) active: {}",
                    active_packs.len(),
                    names.join(", ")
                ),
                evidence: None,
                passed: true,
                note: None,
            });
        }

        let report = self.run_logic_node(input, &active_packs).await?;
        let telemetry: TelemetryResult = extractor::extract(&report.analytical_matrix)?;

        entries.push(TraceEntry {
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
        });

        Ok((telemetry, entries))
    }

    async fn run_logic_node(
        &self,
        input: &str,
        active_packs: &[&'static ContextPack],
    ) -> Result<LogicReport> {
        let (system_prompt, payload) =
            adaptor::prepare(&self.soul.logic_system_prompt, input, active_packs);

        let raw_response = self
            .engine
            .generate(&system_prompt, &payload)
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
