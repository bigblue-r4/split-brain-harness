pub mod adaptor;
pub mod audit;
pub mod backends;
pub mod capability;
pub mod code_gen;
pub mod config;
pub use config::validate_config;
pub mod context_packs;
pub mod extractor;
pub mod generative_forge;
pub mod harness;
pub mod input_validation;
pub mod normalizer;
pub mod policy;
pub mod rag;
pub mod regenerative_forge;
pub mod reputation;
pub mod security;
#[cfg(feature = "serve")]
pub mod serve;
pub mod session_log;
pub mod soul;
pub mod static_analysis;
pub mod tool_forge;
pub mod tool_memory;
pub mod transformer;
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

    let t = if let Some(ref path) = config.context_path {
        let mut corpus = rag::ContextCorpus::embedded();
        match rag::ContextCorpus::load(path) {
            Ok(extra) => corpus.merge(extra),
            Err(e) => eprintln!("warning: could not load context path {path:?}: {e}"),
        }
        transformer::SplitBrainTransformer::with_corpus(
            loaded_soul,
            corpus,
            transformer::TransformPolicy::default(),
        )
    } else {
        transformer::SplitBrainTransformer::new(loaded_soul)
    };

    let h = harness::Harness::new_with_transformer(t, engine.as_ref(), config);
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
