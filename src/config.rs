use crate::types::{BackendType, Config, VerifyMode};
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct FileConfig {
    backend: Option<String>,
    endpoint: Option<String>,
    model_name: Option<String>,
    soul_path: Option<String>,
    api_key: Option<String>,
    verify_mode: Option<String>,
    timeout_secs: Option<u64>,
    memory_path: Option<String>,
    serve_key: Option<String>,
    serve_rate_limit: Option<u32>,
    serve_max_body_bytes: Option<usize>,
}

fn load_file_config() -> FileConfig {
    let path = std::env::var("SBH_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    match std::fs::read_to_string(&path) {
        Ok(c) => toml::from_str(&c).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

pub fn parse_backend(s: &str) -> (BackendType, &'static str) {
    match s {
        "openai-compat" => (BackendType::OpenAiCompat, "http://localhost:8080"),
        "anthropic" => (BackendType::Anthropic, "https://api.anthropic.com"),
        "local-embedded" => (BackendType::LocalEmbedded, ""),
        _ => (BackendType::OllamaNative, "http://localhost:11434"),
    }
}

pub fn parse_verify_mode(s: &str) -> VerifyMode {
    match s {
        "llm" => VerifyMode::Llm,
        "none" => VerifyMode::None,
        _ => VerifyMode::Deterministic,
    }
}

/// Build Config from env vars → config.toml → hardcoded defaults.
pub fn build_config() -> Config {
    let file = load_file_config();
    let backend_str = std::env::var("SBH_BACKEND")
        .ok()
        .or(file.backend)
        .unwrap_or_else(|| "ollama-native".to_string());
    let (backend, default_ep) = parse_backend(&backend_str);
    let default_model = match &backend {
        BackendType::Anthropic => "claude-sonnet-4-6",
        _ => "llama3.2:3b",
    };
    Config {
        backend,
        endpoint: std::env::var("SBH_ENDPOINT")
            .ok()
            .or(file.endpoint)
            .unwrap_or_else(|| default_ep.to_string()),
        model_name: std::env::var("SBH_MODEL")
            .ok()
            .or(file.model_name)
            .unwrap_or_else(|| default_model.to_string()),
        soul_path: std::env::var("SBH_SOUL_PATH")
            .ok()
            .or(file.soul_path)
            .unwrap_or_default(),
        api_key: std::env::var("SBH_API_KEY").ok().or(file.api_key),
        verify_mode: std::env::var("SBH_VERIFY")
            .ok()
            .or(file.verify_mode)
            .map(|s| parse_verify_mode(&s))
            .unwrap_or_default(),
        timeout_secs: std::env::var("SBH_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.timeout_secs)
            .unwrap_or(120),
        dump_prompt: false,
        dump_raw: false,
        memory_path: std::env::var("SBH_MEMORY_PATH").ok().or(file.memory_path),
        serve_key: std::env::var("SBH_SERVE_KEY").ok().or(file.serve_key),
        serve_rate_limit: std::env::var("SBH_SERVE_RATE")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.serve_rate_limit)
            .unwrap_or(60),
        serve_max_body_bytes: std::env::var("SBH_SERVE_MAX_BODY")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.serve_max_body_bytes)
            .unwrap_or(1_048_576),
    }
}
