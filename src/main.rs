use anyhow::{anyhow, Result};
use split_brain_harness::{analyze, types::{BackendType, Config}};
use std::io::Read;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Parse minimal CLI: [--raw] [input text | --stdin]
    let (raw_output, input) = parse_args(&args)?;

    let config = config_from_env();

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

/// Build Config from environment variables with sensible defaults.
/// In a real deployment this would read a config.toml instead.
fn config_from_env() -> Config {
    let backend_str = std::env::var("SBH_BACKEND")
        .unwrap_or_else(|_| "ollama-native".to_string());

    let (backend, default_endpoint) = match backend_str.as_str() {
        "openai-compat"  => (BackendType::OpenAiCompat,  "http://localhost:8080"),
        "anthropic"      => (BackendType::Anthropic,     "https://api.anthropic.com"),
        "local-embedded" => (BackendType::LocalEmbedded, ""),
        _                => (BackendType::OllamaNative,  "http://localhost:11434"),
    };

    let default_model = match &backend {
        BackendType::Anthropic => "claude-sonnet-4-6",
        _                      => "llama3.2:3b",
    };

    Config {
        backend,
        endpoint:   std::env::var("SBH_ENDPOINT")
            .unwrap_or_else(|_| default_endpoint.to_string()),
        model_name: std::env::var("SBH_MODEL")
            .unwrap_or_else(|_| default_model.to_string()),
        soul_path:  std::env::var("SBH_SOUL_PATH")
            .unwrap_or_default(),
        api_key:    std::env::var("SBH_API_KEY").ok(),
    }
}
