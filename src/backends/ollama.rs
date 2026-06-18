use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use super::InferenceEngine;

pub struct OllamaNativeEngine {
    pub endpoint: String,
    pub model:    String,
    pub client:   Client,
}

#[async_trait]
impl InferenceEngine for OllamaNativeEngine {
    async fn generate(&self, system_prompt: &str, prompt_payload: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user",   "content": prompt_payload }
            ],
            "think":  false,
            "stream": false,
            "options": { "temperature": 0.1, "num_predict": 600 }
        });

        let resp = self.client
            .post(format!("{}/api/chat", self.endpoint.trim_end_matches('/')))
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

        json["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "missing content field in Ollama response".into())
    }
}
