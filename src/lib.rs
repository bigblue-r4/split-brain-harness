// v2 clean-core: the foundational typed model lives in sbh-core; re-exported so
// `crate::{types,capability,input_validation,extractor}` and the
// `split_brain_harness::*` paths keep resolving unchanged.
pub use sbh_core::{capability, extractor, input_validation, types};

// v2: append-only JSONL stores extracted to sbh-store; re-exported so
// `crate::{audit,session_log,calibration}` paths resolve unchanged.
pub use sbh_store::{audit, calibration, session_log};

// v2: LLM backends extracted to sbh-llm; re-exported so `crate::backends` and
// `split_brain_harness::backends` resolve unchanged.
pub use sbh_llm as backends;

// v2: shared safety mid-layer extracted to sbh-safety; re-exported so
// `crate::{soul,security,policy}` paths resolve unchanged.
pub use sbh_safety::{policy, security, soul};

// v2: the Ephemeral Tool Forge extracted to sbh-forge; re-exported so
// `crate::{tool_forge,tool_memory,...}` and `split_brain_harness::*` resolve unchanged.
pub use sbh_forge::{
    code_gen, generative_forge, regenerative_forge, reputation, static_analysis, tool_forge,
    tool_memory, wasm_forge,
};

pub mod adaptor;
pub mod advocate;
pub mod arbitrator;
pub mod config;
pub use config::validate_config;
pub mod context_packs;
pub mod formal;
pub mod harness;
// v2: extracted to the sbh-normalize crate; re-exported so `crate::normalizer`
// and `split_brain_harness::normalizer` keep resolving unchanged.
pub use sbh_normalize as normalizer;
pub mod rag;
#[cfg(feature = "serve")]
pub mod serve;
pub mod tool_risk;
pub mod transformer;
pub mod verifier;
pub mod visualize;

use anyhow::Result;
use types::{Config, HarnessResult};

/// Top-level convenience function. Loads soul + backend from config,
/// runs the full two-stage pipeline, returns a HarnessResult.
pub async fn analyze(input: &str, config: &Config) -> Result<HarnessResult> {
    let loaded_soul = soul::load(Some(&config.soul_path))?;
    let engine = backends::init_engine(config);

    let policy = transformer::TransformPolicy {
        request_rationale: config.request_rationale,
        ..Default::default()
    };
    let t = if let Some(ref path) = config.context_path {
        let mut corpus = rag::ContextCorpus::embedded();
        match rag::ContextCorpus::load(path) {
            Ok(extra) => corpus.merge(extra),
            Err(e) => eprintln!("warning: could not load context path {path:?}: {e}"),
        }
        transformer::SplitBrainTransformer::with_corpus(loaded_soul, corpus, policy)
    } else {
        let mut t = transformer::SplitBrainTransformer::new(loaded_soul);
        t.policy = policy;
        t
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
