use super::InferenceEngine;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

pub struct AnthropicEngine {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub client: Client,
}

#[async_trait]
impl InferenceEngine for AnthropicEngine {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String> {
        if self.api_key.is_empty() {
            return Err("SBH_API_KEY is required for the Anthropic backend".into());
        }

        let body = json!({
            "model": self.model,
            "max_tokens": 2048,
            "system": system_prompt,
            "messages": [
                { "role": "user", "content": prompt_payload }
            ]
        });

        let resp = self
            .client
            .post(format!(
                "{}/v1/messages",
                self.endpoint.trim_end_matches('/')
            ))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status();
        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

        if !status.is_success() {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("Anthropic API error {status}: {msg}"));
        }

        json["content"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "missing content[0].text in Anthropic response".into())
    }
}
