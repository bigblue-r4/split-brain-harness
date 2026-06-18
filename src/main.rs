use anyhow::{anyhow, Result};
use serde::Deserialize;
use split_brain_harness::{
    analyze,
    types::{BackendType, Config},
};
use std::io::Read;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Parse minimal CLI: [--raw] [input text | --stdin]
    let (raw_output, input) = parse_args(&args)?;

    let config = build_config();

    let result = analyze(&input, &config).await?;

    if raw_output {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    Ok(())
}

fn parse_args(args: &[String]) -> Result<(bool, String)> {
    let raw = args.contains(&"--raw".to_string());

    if args.contains(&"--stdin".to_string()) {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        return Ok((raw, input.trim().to_string()));
    }

    // Collect positional args (anything not starting with --)
    let positional: Vec<&str> = args[1..]
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .collect();

    if positional.is_empty() {
        return Err(anyhow!(
            "Usage: split-brain-harness [--raw] \"input text\"\n\
             Usage: echo \"input\" | split-brain-harness --stdin [--raw]"
        ));
    }

    Ok((raw, positional.join(" ")))
}

// ---------------------------------------------------------------------------
// Config loading — priority: env vars > config.toml > hardcoded defaults
// ---------------------------------------------------------------------------

/// Partial config loaded from a toml file. All fields optional so a sparse
/// file is valid — missing keys fall through to env vars or defaults.
#[derive(Deserialize, Default)]
struct FileConfig {
    backend: Option<String>,
    endpoint: Option<String>,
    model_name: Option<String>,
    soul_path: Option<String>,
    api_key: Option<String>,
}

/// Load a FileConfig from disk. Path: SBH_CONFIG env var → ./config.toml.
/// Silently returns an empty FileConfig if the file is absent.
fn load_file_config() -> FileConfig {
    let path = std::env::var("SBH_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

fn parse_backend(s: &str) -> (BackendType, &'static str) {
    match s {
        "openai-compat" => (BackendType::OpenAiCompat, "http://localhost:8080"),
        "anthropic" => (BackendType::Anthropic, "https://api.anthropic.com"),
        "local-embedded" => (BackendType::LocalEmbedded, ""),
        _ => (BackendType::OllamaNative, "http://localhost:11434"),
    }
}

fn build_config() -> Config {
    let file = load_file_config();

    // Env var wins; file config is the fallback; hardcoded default is last resort.
    let backend_str = std::env::var("SBH_BACKEND")
        .ok()
        .or(file.backend)
        .unwrap_or_else(|| "ollama-native".to_string());

    let (backend, default_endpoint) = parse_backend(&backend_str);

    let default_model = match &backend {
        BackendType::Anthropic => "claude-sonnet-4-6",
        _ => "llama3.2:3b",
    };

    Config {
        backend,
        endpoint: std::env::var("SBH_ENDPOINT")
            .ok()
            .or(file.endpoint)
            .unwrap_or_else(|| default_endpoint.to_string()),
        model_name: std::env::var("SBH_MODEL")
            .ok()
            .or(file.model_name)
            .unwrap_or_else(|| default_model.to_string()),
        soul_path: std::env::var("SBH_SOUL_PATH")
            .ok()
            .or(file.soul_path)
            .unwrap_or_default(),
        api_key: std::env::var("SBH_API_KEY").ok().or(file.api_key),
    }
}
