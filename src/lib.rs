pub mod backends;
pub mod extractor;
pub mod harness;
pub mod soul;
pub mod types;

use anyhow::Result;
use types::{Config, TelemetryResult};

/// Top-level convenience function. Loads soul + backend from config,
/// runs the full pipeline, returns a TelemetryResult.
pub async fn analyze(input: &str, config: &Config) -> Result<TelemetryResult> {
    let loaded_soul = soul::load(Some(&config.soul_path))?;
    let engine = backends::init_engine(config);
    let h = harness::Harness::new(loaded_soul, engine.as_ref());
    h.analyze(input).await
}
