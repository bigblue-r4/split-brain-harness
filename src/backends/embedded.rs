use async_trait::async_trait;
use super::InferenceEngine;

pub struct LocalEmbeddedEngine {
    pub model_identifier: String,
}

#[async_trait]
impl InferenceEngine for LocalEmbeddedEngine {
    async fn generate(&self, _system_prompt: &str, _prompt_payload: &str) -> Result<String, String> {
        Err(format!(
            "local-embedded backend not yet implemented (model: {}). \
             Use ollama-native or openai-compat.",
            self.model_identifier
        ))
    }
}
