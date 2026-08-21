use async_trait::async_trait;
use reqwest::Client;
use sbh_core::types::{BackendType, Config};

pub mod anthropic;
pub mod embedded;
pub mod ollama;
pub mod openai;

#[async_trait]
pub trait InferenceEngine: Send + Sync {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String>;
}

/// Which hemisphere / stage an inference call belongs to. Roles exist so the
/// two hemispheres can run on different models; with no overrides configured
/// every role resolves to `config.model_name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Stage 1 — the proposer hemisphere (and the advocate stage, which argues
    /// against the proposer's own telemetry).
    Proposer,
    /// Stage 2 — the verifier hemisphere.
    Verifier,
    /// The Reconcile adjudicator, called only when the disagreement structure
    /// matches a high-risk fingerprint.
    Adjudicator,
}

/// The model each role will actually call, with overrides resolved.
pub fn resolved_model(config: &Config, role: Role) -> &str {
    let override_for = match role {
        Role::Proposer => None,
        Role::Verifier => config.verifier_model_name.as_deref(),
        Role::Adjudicator => config.adjudicator_model_name.as_deref(),
    };
    override_for.unwrap_or(&config.model_name)
}

/// One engine per role. `verifier` / `adjudicator` are `None` when that role
/// resolves to the same model as the proposer, so the common single-model case
/// still builds exactly one backend client.
pub struct Engines {
    proposer: Box<dyn InferenceEngine>,
    verifier: Option<Box<dyn InferenceEngine>>,
    adjudicator: Option<Box<dyn InferenceEngine>>,
}

impl Engines {
    /// Assemble a role set directly. `init_engines` is the normal way in; this
    /// exists for embedders wiring their own engines and for tests.
    pub fn new(
        proposer: Box<dyn InferenceEngine>,
        verifier: Option<Box<dyn InferenceEngine>>,
        adjudicator: Option<Box<dyn InferenceEngine>>,
    ) -> Self {
        Self {
            proposer,
            verifier,
            adjudicator,
        }
    }

    pub fn for_role(&self, role: Role) -> &dyn InferenceEngine {
        match role {
            Role::Proposer => self.proposer.as_ref(),
            Role::Verifier => self.verifier.as_deref().unwrap_or(self.proposer.as_ref()),
            Role::Adjudicator => self
                .adjudicator
                .as_deref()
                .unwrap_or(self.proposer.as_ref()),
        }
    }

    /// True when at least one role runs on a model of its own.
    pub fn is_split(&self) -> bool {
        self.verifier.is_some() || self.adjudicator.is_some()
    }
}

/// Build the per-role engine set. Roles without an override share the proposer
/// engine, so this is a drop-in replacement for `init_engine` when no per-role
/// model is configured.
pub fn init_engines(config: &Config) -> Engines {
    let build = |role: Role| {
        let mut role_config = config.clone();
        role_config.model_name = resolved_model(config, role).to_string();
        init_engine(&role_config)
    };
    let distinct =
        |role: Role| (resolved_model(config, role) != config.model_name).then(|| build(role));

    Engines {
        proposer: build(Role::Proposer),
        verifier: distinct(Role::Verifier),
        adjudicator: distinct(Role::Adjudicator),
    }
}

pub fn init_engine(config: &Config) -> Box<dyn InferenceEngine> {
    let client = Client::builder()
        .pool_max_idle_per_host(10)
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .build()
        .unwrap_or_default();

    match config.backend {
        BackendType::OpenAiCompat => Box::new(openai::OpenAiEngine {
            endpoint: config.endpoint.clone(),
            model: config.model_name.clone(),
            temperature: config.temperature,
            client,
        }),
        BackendType::OllamaNative => Box::new(ollama::OllamaNativeEngine {
            endpoint: config.endpoint.clone(),
            model: config.model_name.clone(),
            temperature: config.temperature,
            client,
        }),
        BackendType::LocalEmbedded => Box::new(embedded::LocalEmbeddedEngine {
            model_identifier: config.model_name.clone(),
        }),
        BackendType::Anthropic => Box::new(anthropic::AnthropicEngine {
            endpoint: config.endpoint.clone(),
            model: config.model_name.clone(),
            api_key: config.api_key.clone().unwrap_or_default(),
            temperature: config.temperature,
            client,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            model_name: "llama3.2:3b".into(),
            ..Default::default()
        }
    }

    #[test]
    fn unset_overrides_resolve_to_model_name() {
        let c = cfg();
        for role in [Role::Proposer, Role::Verifier, Role::Adjudicator] {
            assert_eq!(resolved_model(&c, role), "llama3.2:3b");
        }
        assert!(!init_engines(&c).is_split());
    }

    #[test]
    fn verifier_override_splits_only_the_verifier() {
        let c = Config {
            verifier_model_name: Some("qwen3.5".into()),
            ..cfg()
        };
        assert_eq!(resolved_model(&c, Role::Proposer), "llama3.2:3b");
        assert_eq!(resolved_model(&c, Role::Verifier), "qwen3.5");
        // Unset adjudicator still follows the proposer, not the verifier.
        assert_eq!(resolved_model(&c, Role::Adjudicator), "llama3.2:3b");
        assert!(init_engines(&c).is_split());
    }

    #[test]
    fn override_equal_to_model_name_is_not_a_split() {
        let c = Config {
            verifier_model_name: Some("llama3.2:3b".into()),
            ..cfg()
        };
        assert!(!init_engines(&c).is_split());
    }

    #[test]
    fn adjudicator_can_be_overridden_alone() {
        let c = Config {
            adjudicator_model_name: Some("qwen3.5".into()),
            ..cfg()
        };
        assert_eq!(resolved_model(&c, Role::Verifier), "llama3.2:3b");
        assert_eq!(resolved_model(&c, Role::Adjudicator), "qwen3.5");
        assert!(init_engines(&c).is_split());
    }
}
