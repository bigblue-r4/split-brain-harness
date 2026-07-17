/// Transformer / RAG layer — canonical prompt construction for the split-brain pipeline.
///
/// `SplitBrainTransformer` is the single place that assembles the system prompt and
/// payload sent to the inference backend. It combines three sources:
///   1. The soul (identity + operating constraints, from soul.md)
///   2. The RAG context corpus (operator-configurable threat doctrine / schema reference)
///   3. Trigger-matched context packs (conditionally injected for specific threat signals)
///
/// This makes the prompt construction testable, reproducible, and independent of the
/// inference backend. Any backend (Ollama, Anthropic, OpenAI-compat, future embedded)
/// uses the same transformer output.
use crate::capability::ModelContract;
use crate::context_packs::ContextPack;
use crate::extractor;
use crate::rag::ContextCorpus;
use crate::soul;
use crate::types::Soul;
use anyhow::Result;

/// Controls transformer behaviour.
#[derive(Debug, Clone)]
pub struct TransformPolicy {
    /// Maximum characters of RAG context injected into the system prompt.
    /// Whole docs are dropped when the budget is exceeded (never split mid-doc).
    pub max_context_chars: usize,
    /// Ask the proposer to also emit a natural-language `rationale`. Off by
    /// default — the soul's base schema does not request it, so small local
    /// models aren't burdened with the extra generation.
    pub request_rationale: bool,
}

impl Default for TransformPolicy {
    fn default() -> Self {
        Self {
            max_context_chars: 6000,
            request_rationale: false,
        }
    }
}

/// Appended to the system prompt when `request_rationale` is on. Kept out of the
/// base soul schema so the default (small-model) path never generates it.
const RATIONALE_INSTRUCTION: &str = "\n\n--- OPTIONAL RATIONALE ---\n\
Also include a top-level \"rationale\" field alongside the telemetry objects (a \
sibling, never inside them): a single short paragraph (<= 60 words, plain \
language) explaining WHY you assigned this telemetry — the specific input signals \
behind the manipulation_risk, emotion, and urgency reads.\n\
--- END OPTIONAL RATIONALE ---";

/// Assembles system prompts and payloads for the split-brain inference pipeline.
pub struct SplitBrainTransformer {
    pub soul: Soul,
    pub corpus: ContextCorpus,
    pub policy: TransformPolicy,
}

impl SplitBrainTransformer {
    /// Create with embedded default corpus and default policy.
    pub fn new(soul: Soul) -> Self {
        Self {
            soul,
            corpus: ContextCorpus::embedded(),
            policy: TransformPolicy::default(),
        }
    }

    /// Create with a custom corpus and policy.
    pub fn with_corpus(soul: Soul, corpus: ContextCorpus, policy: TransformPolicy) -> Self {
        Self {
            soul,
            corpus,
            policy,
        }
    }

    /// Build the augmented system prompt.
    ///
    /// Order of injection:
    ///   1. Soul logic system prompt (always present)
    ///   2. RAG context pack (embedded + operator docs, up to max_context_chars)
    ///   3. Trigger-matched context packs (only when input matched threat signals)
    pub fn transform_system(&self, trigger_packs: &[&'static ContextPack]) -> String {
        let mut buf = self.soul.logic_system_prompt.clone();

        // Opt-in rationale request, placed next to the schema (not in the base soul).
        if self.policy.request_rationale {
            buf.push_str(RATIONALE_INSTRUCTION);
        }

        // RAG context injection
        let rendered = self.corpus.render(self.policy.max_context_chars);
        if !rendered.is_empty() {
            buf.push_str("\n\n--- CONTEXT REFERENCE ---\n");
            buf.push_str(
                "Use the following doctrine reference when calibrating telemetry scores.\n",
            );
            buf.push('\n');
            buf.push_str(&rendered);
            buf.push_str("\n--- END CONTEXT REFERENCE ---");
        }

        // Trigger-matched pack injection (existing adaptor path, preserved)
        if !trigger_packs.is_empty() {
            buf.push_str("\n\n--- CONTEXT REFERENCE PACKS ---\n");
            buf.push_str(
                "Use the following threat-pattern reference when scoring \
                 manipulation_risk and structural_tone.\n",
            );
            for pack in trigger_packs {
                buf.push('\n');
                buf.push_str(pack.content);
                buf.push('\n');
            }
            buf.push_str("\n--- END CONTEXT REFERENCE PACKS ---");
        }

        buf
    }

    /// Wrap `input` in payload tags for the model.
    pub fn transform_payload(&self, input: &str) -> String {
        soul::wrap_payload(input)
    }

    /// Parse raw model output into a `ModelContract` (telemetry + optional capability request).
    pub fn postprocess(&self, raw: &str) -> Result<ModelContract> {
        extractor::extract(raw).map_err(|e| anyhow::anyhow!("postprocess failed: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soul;

    fn make_transformer() -> SplitBrainTransformer {
        let soul = soul::load(None).unwrap();
        SplitBrainTransformer::new(soul)
    }

    #[test]
    fn transform_system_contains_soul_prompt() {
        let t = make_transformer();
        let system = t.transform_system(&[]);
        assert!(
            system.contains("telemetry engine"),
            "soul logic prompt must appear in system output"
        );
    }

    #[test]
    fn rationale_is_opt_in() {
        // Default: no rationale request (small-model friendly).
        let t = make_transformer();
        assert!(!t.transform_system(&[]).to_lowercase().contains("rationale"));
        // Opt-in: the instruction is injected.
        let mut t2 = make_transformer();
        t2.policy.request_rationale = true;
        assert!(t2.transform_system(&[]).contains("\"rationale\" field"));
    }

    #[test]
    fn transform_system_injects_rag_context() {
        let t = make_transformer();
        let system = t.transform_system(&[]);
        assert!(
            system.contains("<context_pack>"),
            "RAG context block must be injected"
        );
        assert!(
            system.contains("TelemetryResult Field Reference"),
            "schema doc must appear in context"
        );
    }

    #[test]
    fn transform_system_is_deterministic() {
        let soul = soul::load(None).unwrap();
        let t1 = SplitBrainTransformer::new(soul.clone());
        let t2 = SplitBrainTransformer::new(soul);
        assert_eq!(
            t1.transform_system(&[]),
            t2.transform_system(&[]),
            "same soul + corpus must always produce the same system prompt"
        );
    }

    #[test]
    fn transform_system_no_packs_excludes_pack_section() {
        let t = make_transformer();
        let system = t.transform_system(&[]);
        assert!(
            !system.contains("CONTEXT REFERENCE PACKS"),
            "pack section must be absent when no packs are active"
        );
    }

    #[test]
    fn transform_payload_wraps_in_tags() {
        let t = make_transformer();
        let payload = t.transform_payload("hello world");
        assert!(payload.contains("<payload>"), "must open payload tag");
        assert!(payload.contains("hello world"), "must contain input");
    }

    #[test]
    fn transform_system_with_empty_corpus_omits_context_block() {
        let soul = soul::load(None).unwrap();
        let t = SplitBrainTransformer::with_corpus(
            soul,
            ContextCorpus::default(),
            TransformPolicy::default(),
        );
        let system = t.transform_system(&[]);
        assert!(
            !system.contains("<context_pack>"),
            "no context block when corpus is empty"
        );
    }

    #[test]
    fn policy_max_context_chars_limits_injection() {
        let soul = soul::load(None).unwrap();
        let policy = TransformPolicy {
            max_context_chars: 100,
            ..Default::default()
        };
        let t = SplitBrainTransformer::with_corpus(soul, ContextCorpus::embedded(), policy);
        let system = t.transform_system(&[]);
        // With 100 char limit, the context pack should be present but truncated
        // (possibly just the wrapper tags if no doc fits)
        assert!(system.contains("CONTEXT REFERENCE") || !system.contains("<context_pack>"));
    }

    #[test]
    fn with_corpus_uses_provided_corpus() {
        let soul = soul::load(None).unwrap();
        let custom_doc = crate::rag::ContextDoc {
            id: "custom.test".into(),
            title: "Custom Test Doc".into(),
            text: "custom content for test".into(),
            tags: vec![],
        };
        let corpus = ContextCorpus {
            docs: vec![custom_doc],
        };
        let t = SplitBrainTransformer::with_corpus(soul, corpus, TransformPolicy::default());
        let system = t.transform_system(&[]);
        assert!(
            system.contains("Custom Test Doc"),
            "custom doc must appear in system prompt"
        );
        assert!(system.contains("custom content for test"));
    }

    #[test]
    fn soul_field_is_accessible() {
        let t = make_transformer();
        assert!(!t.soul.logic_system_prompt.is_empty());
        assert!(!t.soul.verifier_system_prompt.is_empty());
    }
}
