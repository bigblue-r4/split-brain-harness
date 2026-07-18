use sbh_core::types::{BackendType, Config};
use async_trait::async_trait;
use reqwest::Client;

pub mod anthropic;
pub mod embedded;
pub mod ollama;
pub mod openai;

#[async_trait]
pub trait InferenceEngine: Send + Sync {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String>;
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
