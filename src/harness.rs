use anyhow::{anyhow, Result};
use crate::backends::InferenceEngine;
use crate::extractor;
use crate::soul;
use crate::types::{RawInput, LogicReport, TelemetryResult, Soul};

/// Core pipeline. Stateless — one instance per analysis call is fine,
/// or hold one Harness and call analyze() repeatedly.
pub struct Harness<'e> {
    soul:   Soul,
    engine: &'e dyn InferenceEngine,
}

impl<'e> Harness<'e> {
    pub fn new(soul: Soul, engine: &'e dyn InferenceEngine) -> Self {
        Self { soul, engine }
    }

    /// Run the full pipeline for a single raw input string.
    /// Returns a fully validated TelemetryResult or a descriptive error.
    pub async fn analyze(&self, input: &str) -> Result<TelemetryResult> {
        let raw    = self.run_logic_node(RawInput(input.to_string())).await?;
        let result = self.extract_result(raw)?;
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Pipeline stages — named to match the state transition types in types.rs
    // -----------------------------------------------------------------------

    async fn run_logic_node(&self, input: RawInput) -> Result<LogicReport> {
        let payload = soul::wrap_payload(&input.0);

        let raw_response = self.engine
            .generate(&self.soul.logic_system_prompt, &payload)
            .await
            .map_err(|e| anyhow!("inference backend error: {}", e))?;

        if raw_response.trim().is_empty() {
            return Err(anyhow!("model returned an empty response"));
        }

        Ok(LogicReport { analytical_matrix: raw_response })
    }

    fn extract_result(&self, report: LogicReport) -> Result<TelemetryResult> {
        extractor::extract(&report.analytical_matrix)
    }
}
