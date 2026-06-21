pub mod adaptor;
pub mod backends;
pub mod context_packs;
pub mod extractor;
pub mod harness;
pub mod soul;
pub mod types;
pub mod verifier;

use anyhow::Result;
use types::{Config, HarnessResult};

/// Top-level convenience function. Loads soul + backend from config,
/// runs the full two-stage pipeline, returns a HarnessResult.
pub async fn analyze(input: &str, config: &Config) -> Result<HarnessResult> {
    let loaded_soul = soul::load(Some(&config.soul_path))?;
    let engine = backends::init_engine(config);
    let h = harness::Harness::new(loaded_soul, engine.as_ref(), config);
    h.analyze(input).await
}
