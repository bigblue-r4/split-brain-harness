pub mod adaptor;
pub mod backends;
pub mod capability;
pub mod code_gen;
pub mod config;
pub mod context_packs;
pub mod extractor;
pub mod generative_forge;
pub mod harness;
pub mod input_validation;
pub mod policy;
pub mod soul;
pub mod static_analysis;
pub mod tool_forge;
pub mod tool_memory;
pub mod types;
pub mod verifier;
pub mod wasm_forge;

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

/// Build the augmented system prompt and payload for `input` without calling
/// the model. Used by `--dump-prompt` to print and exit before any API call.
pub fn prepare_prompt(input: &str, config: &Config) -> Result<(String, String)> {
    let loaded_soul = soul::load(Some(&config.soul_path))?;
    let selections = adaptor::select_packs_with_evidence(input);
    let active_packs: Vec<&'static context_packs::ContextPack> =
        selections.iter().map(|s| s.pack).collect();
    Ok(adaptor::prepare(
        &loaded_soul.logic_system_prompt,
        input,
        &active_packs,
    ))
}
