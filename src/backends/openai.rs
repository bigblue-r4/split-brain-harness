use super::InferenceEngine;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

pub struct OpenAiEngine {
    pub endpoint: String,
    pub model: String,
    pub client: Client,
}

#[async_trait]
impl InferenceEngine for OpenAiEngine {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user",   "content": prompt_payload }
            ],
            "temperature": 0.1,
            "max_tokens": 2048
        });

        let resp = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.endpoint.trim_end_matches('/')
            ))
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

        json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "missing content field in OpenAI response".into())
    }
}
